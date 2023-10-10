use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{Error, ErrorKind},
    mem::size_of,
    os::unix::prelude::FileExt,
    sync::Arc,
    usize,
};
use tokio::{
    io::{AsyncSeekExt, AsyncWriteExt},
    sync::RwLock,
};

use crate::{
    bitmap::{self, alloc_bit, dealloc_data_bit, BitmapType},
    fs_constants::*,
    inode::Inode,
    simple_fs::SFS,
};

pub type BlockIDType = u32;
#[derive(Clone, Debug)]
pub struct Block {
    pub block_id: usize,         //块编号
    pub bytes: [u8; BLOCK_SIZE], //块的字节内容
    pub modified: bool,          //是否修改位，用于缓存写入
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        self.block_id == other.block_id
    }
}

impl Block {
    pub fn modify_bytes<F>(&mut self, f: F)
    where
        F: FnOnce(&mut [u8]),
    {
        f(&mut self.bytes);
        self.modified = true;
    }
}

pub struct BlockCacheManager {
    pub block_cache: HashMap<usize, Block>,
}

impl BlockCacheManager {
    pub fn new() -> Self {
        Self {
            block_cache: HashMap::new(),
        }
    }

    /// 将所有块缓存写入磁盘，同时清空缓存
    pub async fn sync_and_clear_cache(&mut self) -> Result<(), Error> {
        let mut file = None;
        for block in self.block_cache.values_mut() {
            if !block.modified {
                continue;
            }

            if file.is_none() {
                file = Some(
                    tokio::fs::OpenOptions::new()
                        .write(true)
                        .open(FS_FILE_NAME)
                        .await?,
                )
            }

            if let Some(file) = &mut file {
                let buf = block.bytes;
                trace!("sync block {}", block.block_id);
                let offset = block.block_id * BLOCK_SIZE;
                let pos = tokio::io::SeekFrom::Start(offset as u64);
                file.seek(pos).await?;
                file.write_all(&buf).await?;
            }
        }

        self.block_cache.clear();
        Ok(())
    }
}

/// 将块读入缓存中
pub async fn read_block_to_cache(block_id: usize) -> Result<(), Error> {
    read_blocks_to_cache(&[block_id]).await
}

/// 批量将块读入缓存中
pub async fn read_blocks_to_cache(block_id_addrs: &[usize]) -> Result<(), Error> {
    let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
    let mut w = blk.write().await;
    let mut file = None;

    for block_id in block_id_addrs {
        if w.block_cache.contains_key(block_id) {
            continue;
        }

        if file.is_none() {
            file = Some(File::open(FS_FILE_NAME)?);
        }

        let mut block = Block {
            block_id: *block_id,
            bytes: [0; BLOCK_SIZE],
            modified: false,
        };

        let offset = block_id * BLOCK_SIZE;
        if let Some(file) = &mut file {
            if file.read_exact_at(&mut block.bytes, offset as u64).is_err() {
                let e = format!("cannot read buffer at {}", offset);
                error!("{}", e);
                return Err(Error::new(ErrorKind::AddrNotAvailable, e));
            }
            w.block_cache.insert(*block_id, block);
            trace!("block {} push to cache", block_id);
        }
    }
    Ok(())
}

/// 获取指定块中的某一段缓存
pub async fn get_block_buffer(
    block_id: usize,
    start_byte: usize,
    end_byte: usize,
) -> Result<Vec<u8>, Error> {
    let buffers = get_blocks_buffers(&[(block_id, start_byte, end_byte)]).await?;
    Ok(buffers[0].clone())
}

/// 批量获取指定块中的某一段缓存
pub async fn get_blocks_buffers(
    blocks_args: &[(usize, usize, usize)],
) -> Result<Vec<Vec<u8>>, Error> {
    let ids: Vec<_> = blocks_args.iter().map(|(id, _, _)| *id).collect();
    read_blocks_to_cache(&ids).await?;
    let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
    let bcm = blk.read().await;
    let mut buffers = Vec::new();
    for (block_id, start, end) in blocks_args {
        let block = match bcm.block_cache.get(block_id) {
            Some(block) => block,
            None => {
                // 可能会因为他人持有写锁，写完后清空了缓存导致读不到缓存，所以要重读
                info!("re-read caches");
                read_blocks_to_cache(&ids).await?;
                bcm.block_cache.get(block_id).unwrap()
            }
        };
        buffers.push(block.bytes[*start..*end].to_vec());
    }
    Ok(buffers)
}

/// 将文件内容分组批量写入缓存
pub async fn write_file_content_to_blocks(
    contents: &[String],
    block_ids: &[usize],
) -> Result<(), Error> {
    trace!("write block{:?}", block_ids);
    // 当块不在缓存中时 读入缓存
    read_blocks_to_cache(block_ids).await?;
    let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
    let mut bcm = blk.write().await;
    for (i, block_id) in block_ids.iter().enumerate() {
        let block = bcm.block_cache.get_mut(block_id).unwrap();
        let content = contents[i].clone();
        assert!(BLOCK_SIZE >= content.len());
        block.modify_bytes(|bytes_arr| {
            let end = content.len();
            bytes_arr[..end].clone_from_slice(content.as_bytes());
        });
    }
    Ok(())
}

/// 将`object`序列化并写入指定的`block_id`中，
/// 用`start_byte`指示出该`object`会在块中的字节起始位置
pub async fn write_block<T: serde::Serialize>(
    object: &T,
    block_id: usize,
    start_byte: usize,
) -> Result<(), Error> {
    write_blocks(&[(object, block_id, start_byte)]).await
}

/// 批量将object写入块中， args为（object，block_id, start_byte）数组
pub async fn write_blocks<T: serde::Serialize>(
    object_args: &[(&T, usize, usize)],
) -> Result<(), Error> {
    let ids: Vec<_> = object_args
        .iter()
        .map(|(_, block_id, _)| *block_id)
        .collect();
    read_blocks_to_cache(&ids).await?;
    let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
    let mut bcm = blk.write().await;

    for (object, block_id, start_byte) in object_args {
        trace!("write block{}", *block_id);
        let block = bcm.block_cache.get_mut(block_id).unwrap();
        // 将 object 序列化
        match bincode::serialize(*object) {
            Ok(obj_bytes) => {
                let end_byte = obj_bytes.len() + start_byte;
                assert!(end_byte <= BLOCK_SIZE);
                trace!("write block{}, len {}B", block_id, obj_bytes.len());
                block.modify_bytes(|bytes_arr| {
                    bytes_arr[*start_byte..end_byte].clone_from_slice(&obj_bytes);
                });
            }
            Err(err) => {
                let e = format!("cannot serialize:{}", err);
                error!("{e}");
                return Err(Error::new(ErrorKind::Other, e));
            }
        }
    }
    Ok(())
}

/// 尝试插入一个object到磁盘中
pub async fn insert_object<T: Serialize + Default + DeserializeOwned + PartialEq>(
    object: &T,
    inode: &mut Inode,
) -> Result<(), Error> {
    let all_blocks = get_all_blocks(inode).await?;
    for (_, id, _) in &all_blocks {
        if try_insert_to_block(object, *id as usize).await.is_ok() {
            return Ok(());
        }
        // 如果该块没有空余，继续找
    }
    // 没有空余的，申请
    let last_level = &all_blocks.last().unwrap().0;
    match *last_level {
        BlockLevel::Direct => {
            //申请一个块
            for i in 0..DIRECT_BLOCK_NUM {
                if inode.addr[i] == 0 {
                    let new_block_id = alloc_bit(BitmapType::Data).await?;
                    trace!("add a new direct block {}", new_block_id);
                    // 将地址写回inode中
                    inode.addr[i] = new_block_id;
                    write_block(object, new_block_id as usize, 0).await?;
                    return Ok(());
                }
            }
            // 直接块用完了，要申请一个新的一级块
            let new_first_id = alloc_bit(BitmapType::Data).await?;
            trace!("add a new first block {}", new_first_id);
            // 将一级地址写回inode中
            inode.set_first_id(new_first_id);
            alloc_new_in_first(new_first_id as usize, object).await
        }
        BlockLevel::FirstIndirect => {
            // 一级间接块的已有的所有直接块没有空间了
            if all_blocks.len() < FISRT_MAX + DIRECT_BLOCK_NUM {
                // 一级间接块本身还有空间，直接附加
                alloc_new_in_first(inode.get_first_id(), object).await
            } else {
                // 一级块没空间了，要找二级块（返回的是最后一块一级块）
                // 申请一块新的二级块
                let new_second_id = alloc_bit(BitmapType::Data).await?;
                // 将二级地址写回inode中
                inode.set_second_id(new_second_id);
                alloc_new_in_second(new_second_id as usize, object).await
            }
        }
        BlockLevel::SecondIndirect => {
            if all_blocks.len() < SECOND_MAX + FISRT_MAX + DIRECT_BLOCK_NUM {
                // 最后非空块填满了，申请一块新的一级块
                return alloc_new_in_second(inode.get_second_id(), object).await;
            }
            // 超限
            Err(Error::new(ErrorKind::OutOfMemory, "no valid block"))
        }
    }
}

/// 批量清空block的内容
pub async fn clear_blocks(block_ids: &[usize]) -> Result<(), Error> {
    read_blocks_to_cache(block_ids).await?;
    let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
    let mut bcm = blk.write().await;

    for block_id in block_ids {
        let block = bcm.block_cache.get_mut(block_id).unwrap();
        block.bytes = [0; BLOCK_SIZE];
        block.modified = true;
    }
    Ok(())
}

/// 在二级块中alloc一块新的一级块，并在新的一级块中alloc一块新块
async fn alloc_new_in_second<T: Serialize>(second_id: usize, object: &T) -> Result<(), Error> {
    let new_first_block = alloc_bit(BitmapType::Data).await?;
    alloc_new_in_first(new_first_block as usize, object).await?;
    try_insert_to_block(&new_first_block, second_id).await?;
    Ok(())
}

/// 在新的一级块中alloc一块新块
async fn alloc_new_in_first<T: Serialize>(first_id: usize, object: &T) -> Result<(), Error> {
    // 申请一块新块
    let new_block_id = alloc_bit(BitmapType::Data).await?;
    trace!("add a new block {}", new_block_id);
    // 将object 写入新块
    write_block(object, new_block_id as usize, 0).await?;
    // 把新块id附加到一级块
    try_insert_to_block(&new_block_id, first_id).await
}

// 尝试写入该block的空闲位置，失败（空间不足）则返回Err
async fn try_insert_to_block<T: Serialize + Default + DeserializeOwned + PartialEq>(
    object: &T,
    block_id: usize,
) -> Result<(), Error> {
    let size = size_of::<T>();
    // 搜索该块的每一个object
    for i in 0..BLOCK_SIZE / size {
        let start = i * size;
        let end = start + size;
        // 获得object大小的buffer
        let buffer = get_block_buffer(block_id, start, end).await?;
        // 如果是默认值（空余位置）
        let obj: T = deserialize(&buffer)?;
        if obj == T::default() {
            write_block(object, block_id, start).await?;
            return Ok(());
        }
    }
    // block 没有足够空间
    Err(Error::new(ErrorKind::OutOfMemory, "no enough blocks"))
}

/// 获取直接块
async fn get_direct_blocks(id: &[BlockIDType]) -> Result<Vec<Vec<u8>>, Error> {
    let args: Vec<_> = id.iter().map(|id| (*id as usize, 0, BLOCK_SIZE)).collect();
    get_blocks_buffers(&args).await
}

/// 获取一个一级块所包含的所有直接块
async fn get_blocks_of_first(
    first_id: BlockIDType,
) -> Result<Vec<(BlockLevel, BlockIDType, Vec<u8>)>, Error> {
    get_block_of_first_arr(&[first_id]).await
}

async fn get_block_of_first_arr(
    first_ids: &[BlockIDType],
) -> Result<Vec<(BlockLevel, BlockIDType, Vec<u8>)>, Error> {
    // 计算偏移量，存入数组
    let mut first_args = Vec::new();
    for first_id in first_ids {
        for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
            let start = i * BLOCK_ADDR_SIZE;
            let end = start + BLOCK_ADDR_SIZE;
            first_args.push((*first_id as usize, start, end));
        }
    }
    // 取出一级块内所有地址的buffer
    let buffers = get_blocks_buffers(&first_args).await?;
    // 反序列化得到直接块id数组，和偏移量
    let mut direct_args = Vec::new();
    for addr_buff in buffers {
        let direct_id: BlockIDType = deserialize(&addr_buff)?;
        if direct_id == 0 {
            continue; // 为空
        }
        direct_args.push((direct_id as usize, 0_usize, BLOCK_SIZE));
    }
    // 取出所有直接块，并做好标记
    let mut v = Vec::new();
    let direct_buffers = get_blocks_buffers(&direct_args).await?;
    for (i, buffer) in direct_buffers.into_iter().enumerate() {
        v.push((
            BlockLevel::FirstIndirect,
            direct_args[i].0 as BlockIDType,
            buffer,
        ));
    }
    Ok(v)
}

/// 获取一个二级块所包含的所有直接块
async fn get_blocks_of_second(
    second_id: BlockIDType,
) -> Result<Vec<(BlockLevel, BlockIDType, Vec<u8>)>, Error> {
    // 计算偏移量，存入数组
    let mut second_args = Vec::new();
    for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
        let start = i * BLOCK_ADDR_SIZE;
        let end = start + BLOCK_ADDR_SIZE;
        second_args.push((second_id as usize, start, end));
    }
    let first_addr_buffers = get_blocks_buffers(&second_args).await?;
    // 反序列化得到一级块id数组，和偏移量
    let mut first_ids = Vec::new();
    for addr_buff in first_addr_buffers {
        let first_id: BlockIDType = deserialize(&addr_buff)?;
        if first_id == 0 {
            break; // 为空
        }
        first_ids.push(first_id);
    }
    // 从一级块中取出直接块
    let mut buffers = get_block_of_first_arr(&first_ids).await?;
    let mut v = Vec::new();
    for (level, _, _) in &mut buffers {
        *level = BlockLevel::SecondIndirect;
    }
    v.append(&mut buffers);
    Ok(v)
}

/// 获取所有直接块（包含空块，即便地址有效）
pub async fn get_all_blocks(
    inode: &Inode,
) -> Result<Vec<(BlockLevel, BlockIDType, Vec<u8>)>, Error> {
    let mut v = Vec::new();
    // 直接块
    let mut l = DIRECT_BLOCK_NUM;
    for i in 0..DIRECT_BLOCK_NUM {
        if inode.addr[i] == 0 {
            l = i;
            break;
        }
    }
    let directs = get_direct_blocks(&inode.addr[..l]).await?;
    let mut direct_args = Vec::new();
    for (i, buffer) in directs.into_iter().enumerate() {
        direct_args.push((BlockLevel::Direct, inode.addr[i], buffer));
    }
    v.append(&mut direct_args);
    if l < DIRECT_BLOCK_NUM {
        return Ok(v);
    }

    // 一级
    let first_id = inode.get_first_id() as BlockIDType;
    if first_id == 0 {
        return Ok(v);
    }
    v.append(&mut get_blocks_of_first(first_id).await?);

    // 二级
    let second_id = inode.get_second_id() as BlockIDType;
    if second_id == 0 {
        return Ok(v);
    }
    v.append(&mut get_blocks_of_second(second_id).await?);

    Ok(v)
}

/// 获取所有非空块
pub async fn get_all_valid_blocks(
    inode: &Inode,
) -> Result<Vec<(BlockLevel, BlockIDType, Vec<u8>)>, Error> {
    let mut v = get_all_blocks(inode).await?;
    // 保留非空block
    v.retain(|(_, _, block)| !block_is_empty(block));
    Ok(v)
}

/// 移除一个object，如果这是唯一的object，那么释放这个block
pub async fn remove_object<T: Serialize + Default + PartialEq + DeserializeOwned>(
    object: &T,
    block_id: usize,
    level: BlockLevel,
    inode: &mut Inode,
) -> Result<(), Error> {
    //1.序列化这个block，一一比较
    let size = size_of::<T>();
    let mut exist = false;

    let mut block_args = Vec::new();
    for i in 0..BLOCK_SIZE / size {
        let start = i * size;
        let end = start + size;
        block_args.push((block_id, start, end));
    }
    let buffers = get_blocks_buffers(&block_args).await?;

    for (i, buffer) in buffers.iter().enumerate() {
        if *object == deserialize(buffer)? {
            exist = true;
            // 覆盖该位置
            let start = i * size;
            write_block(&T::default(), block_id, start).await?;
            break;
        }
    }

    if !exist {
        return Err(Error::new(ErrorKind::NotFound, ""));
    }

    //2. 再次序列化，判断是否已空, 如果全空 dealloc
    let block = get_block_buffer(block_id, 0, BLOCK_SIZE).await?;
    if !block_is_empty(&block) {
        return Ok(());
    }

    dealloc_data_bit(block_id).await;
    trace!("dealloc data bit ok");

    match level {
        BlockLevel::Direct => {
            //3.1. 如果是直接块，去inode将地址置空
            for i in 0..DIRECT_BLOCK_NUM {
                if block_id == inode.addr[i] as usize {
                    inode.addr[i] = 0;
                    return Ok(());
                }
            }
            panic!("unreachable");
        }
        BlockLevel::FirstIndirect => {
            //3.2. 如果是在一级块，那么还要清除在一级块中的地址，判断释放这个block addr之后一级块是否已空
            let first_id = inode.get_first_id();
            // 在一级块中清除该块的地址
            remove_block_addr_in_first_block(first_id, block_id).await?;
            inode.set_first_id(0);
        }
        BlockLevel::SecondIndirect => {
            //3.3. 如果是在二级块，判断二级块是否已空
            let second_id = inode.get_second_id();
            let mut first_id = 0;
            let mut start = 0; // 记录二级块中的一级块条目偏移量

            // 首先对二级块的每个一级地址所记录的直接块去清除记录
            let mut second_args = Vec::new();
            for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
                let start = i * BLOCK_ADDR_SIZE;
                let end = start + BLOCK_ADDR_SIZE;
                second_args.push((second_id, start, end));
            }
            let first_addrs = get_blocks_buffers(&second_args).await?;

            for (i, first_addr) in first_addrs.iter().enumerate() {
                first_id = deserialize(first_addr)?;
                if remove_block_addr_in_first_block(first_id, block_id)
                    .await
                    .is_ok()
                {
                    // 找到并清除了，跳出循环
                    start = i * BLOCK_ADDR_SIZE;
                    break;
                }
            }

            // 然后检查找到的那个一级块是否空，空了就清掉那个一级块在二级块中的记录
            let first_block = get_block_buffer(first_id, 0, BLOCK_SIZE).await?;
            if !block_is_empty(&first_block) {
                // 那个一级块还有条目，直接返回
                return Ok(());
            }
            // 在二级块中清除一级块记录
            write_block(&(0 as BlockIDType), second_id, start).await?;

            // 最后检查二级块 如果二级块空了就把二级块也清空
            let second_block = get_block_buffer(second_id, 0, BLOCK_SIZE).await?;
            if !block_is_empty(&second_block) {
                return Ok(());
            }
            // 全空, 释放二级块
            dealloc_data_bit(second_id).await;
            inode.set_second_id(0);
        }
    }
    trace!("remove obj ok");
    Ok(())
}

/// 清除一级块中的直接块地址条目，同时一级块变空时dealloc一级块
async fn remove_block_addr_in_first_block(first_id: usize, block_id: usize) -> Result<(), Error> {
    let mut exist = false;
    let mut first_args = Vec::new();
    for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
        let start = i * BLOCK_ADDR_SIZE;
        let end = start + BLOCK_ADDR_SIZE;
        first_args.push((first_id, start, end));
    }
    let direct_addrs = get_blocks_buffers(&first_args).await?;

    for (i, direct_addr) in direct_addrs.iter().enumerate() {
        // 在一级块中找到了这个块的地址，清除
        if *direct_addr == serialize(&(block_id as BlockIDType))? {
            exist = true;
            let start = i * BLOCK_ADDR_SIZE;
            write_block(&(0 as BlockIDType), first_id, start).await?;
            break;
        }
    }

    if !exist {
        return Err(Error::new(ErrorKind::NotFound, ""));
    }
    let first_block = get_block_buffer(first_id, 0, BLOCK_SIZE).await?;
    if !block_is_empty(&first_block) {
        return Ok(());
    }
    dealloc_data_bit(first_id).await;
    Ok(())
}

/// 判断block是否是全0
pub fn block_is_empty(block: &[u8]) -> bool {
    for b in block {
        if *b != 0 {
            return false;
        }
    }
    true
}

/// 检查data位图对应的区域是否出错
pub async fn check_data_and_fix() -> Result<(), Error> {
    let data_bitmap = bitmap::get_data_bitmaps().await;
    for (i, byte) in data_bitmap.iter().enumerate() {
        for j in 0..8 {
            let mask = 0b10000000 >> j;
            let bit_id = i * 8 + j;
            if bit_id >= DATA_BLOCK_MAX_NUM {
                return Ok(());
            }
            let block_id = bit_id + DATA_START_BLOCK;
            if byte & mask == 1 {
                // 检查对应区域是否为空，为空则置0
                let block = get_block_buffer(block_id, 0, BLOCK_SIZE).await?;
                if block.is_empty() {
                    dealloc_data_bit(block_id).await;
                    info!("fix data bit:{}", bit_id);
                }
            }
        }
    }
    Ok(())
}

//延迟加载全局变量 BLOCK_CACHE_MANAGER
lazy_static! {
    pub static ref BLOCK_CACHE_MANAGER: Arc<RwLock<BlockCacheManager>> =
        Arc::new(RwLock::new(BlockCacheManager::new()));
}

#[derive(Clone, Copy)]
pub enum BlockLevel {
    Direct,
    FirstIndirect,
    SecondIndirect,
}

/// 清空块缓存，写入磁盘中
pub async fn sync_all_block_cache() -> Result<(), Error> {
    Arc::clone(&BLOCK_CACHE_MANAGER)
        .write()
        .await
        .sync_and_clear_cache()
        .await?;
    // 重新读取已写入的信息
    Arc::clone(&SFS).write().await.update().await;
    trace!("sync all blocks ok");
    Ok(())
}

pub fn deserialize<'a, T: Deserialize<'a>>(buffer: &'a [u8]) -> Result<T, Error> {
    bincode::deserialize(buffer).map_err(|err| Error::new(ErrorKind::Other, err))
}

pub fn serialize<T: Serialize>(object: &T) -> Result<Vec<u8>, Error> {
    bincode::serialize(object).map_err(|err| Error::new(ErrorKind::Other, err))
}
