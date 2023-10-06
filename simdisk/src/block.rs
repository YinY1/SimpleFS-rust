use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{Error, ErrorKind},
    mem::size_of,
    os::unix::prelude::FileExt,
    sync::Arc,
};
use tokio::{
    io::{AsyncSeekExt, AsyncWriteExt},
    sync::RwLock,
};

use crate::{
    bitmap::{alloc_bit, dealloc_data_bit, BitmapType},
    fs_constants::*,
    inode::Inode,
    simple_fs::SFS,
};

pub type BlockIDType = u32;
#[derive(Clone, Debug)]
pub struct Block {
    pub block_id: usize,
    pub bytes: [u8; BLOCK_SIZE],
    pub modified: bool,
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

    async fn sync_block(&mut self) -> Result<(), Error> {
        if self.modified {
            if let Ok(mut file) = tokio::fs::OpenOptions::new()
                .write(true)
                .open(FS_FILE_NAME)
                .await
            {
                let buf = self.bytes;
                trace!("drop block {}", self.block_id);
                let offset = self.block_id * BLOCK_SIZE;
                let pos = tokio::io::SeekFrom::Start(offset as u64);
                file.seek(pos).await?;
                file.write_all(&buf).await?;
            }
        }
        Ok(())
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

    pub async fn sync_and_clear_cache(&mut self) -> Result<(), Error> {
        for block in self.block_cache.values_mut() {
            block.sync_block().await?;
        }
        self.block_cache.clear();
        Ok(())
    }
}
/// 将块读入缓存中
pub async fn read_block_to_cache(block_id: usize) -> Result<(), Error> {
    let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
    let mut w = blk.write().await;

    if w.block_cache.contains_key(&block_id) {
        return Ok(());
    }

    let mut block = Block {
        block_id,
        bytes: [0; BLOCK_SIZE],
        modified: false,
    };

    let offset = block_id * BLOCK_SIZE;
    match File::open(FS_FILE_NAME) {
        Ok(file) => {
            if file.read_exact_at(&mut block.bytes, offset as u64).is_err() {
                let e = format!("cannot read buffer at {}", offset);
                error!("{}", e);
                return Err(Error::new(ErrorKind::AddrNotAvailable, e));
            }
        }
        Err(error) => return Err(error),
    }
    w.block_cache.insert(block_id, block);
    trace!("block {} push to cache", block_id);
    Ok(())
}

/// 获取指定块中的某一段缓存
pub async fn get_block_buffer(
    block_id: usize,
    start_byte: usize,
    end_byte: usize,
) -> Result<Vec<u8>, Error> {
    // 当块不在缓存中时 读入缓存
    read_block_to_cache(block_id).await?;

    let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
    let bcm = blk.read().await;
    let block = bcm.block_cache.get(&block_id).unwrap();
    Ok(block.bytes[start_byte..end_byte].to_vec())
}

pub async fn write_file_content_to_block(content: String, block_id: usize) -> Result<(), Error> {
    assert!(BLOCK_SIZE >= content.len());
    trace!("write block{}", block_id);
    // 当块不在缓存中时 读入缓存
    read_block_to_cache(block_id).await?;
    let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
    let mut bcm = blk.write().await;
    let block = bcm.block_cache.get_mut(&block_id).unwrap();
    block.modify_bytes(|bytes_arr| {
        let end = content.len();
        bytes_arr[..end].clone_from_slice(content.as_bytes());
    });
    Ok(())
}

/// 将`object`序列化并写入指定的`block_id`中，
/// 用`start_byte`指示出该`object`会在块中的字节起始位置
pub async fn write_block<T: serde::Serialize>(
    object: &T,
    block_id: usize,
    start_byte: usize,
) -> Result<(), Error> {
    trace!("write block{}", block_id);
    // 当块不在缓存中时 读入缓存
    read_block_to_cache(block_id).await?;
    let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
    let mut bcm = blk.write().await;
    let block = bcm.block_cache.get_mut(&block_id).unwrap();
    // 将 object 序列化
    match bincode::serialize(object) {
        Ok(obj_bytes) => {
            let end_byte = obj_bytes.len() + start_byte;
            trace!("write block{}, len {}B", block_id, obj_bytes.len());
            block.modify_bytes(|bytes_arr| {
                bytes_arr[start_byte..end_byte].clone_from_slice(&obj_bytes);
            });
            Ok(())
        }
        Err(err) => {
            let e = format!("cannot serialize:{}", err);
            error!("{e}");
            Err(Error::new(ErrorKind::Other, e))
        }
    }
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
            alloc_new_first(new_first_id as usize, object).await
        }
        BlockLevel::FirstIndirect => {
            // 一级间接块的已有的所有直接块没有空间了
            if all_blocks.len() < FISRT_MAX + DIRECT_BLOCK_NUM {
                // 一级间接块本身还有空间，直接附加
                alloc_new_first(inode.get_first_id(), object).await
            } else {
                // 一级块没空间了，要找二级块（返回的是最后一块一级块）
                // 申请一块新的二级块
                let new_second_id = alloc_bit(BitmapType::Data).await?;
                // 将二级地址写回inode中
                inode.set_second_id(new_second_id);
                alloc_new_second(object, new_second_id as usize).await
            }
        }
        BlockLevel::SecondIndirect => {
            if all_blocks.len() < SECOND_MAX + FISRT_MAX + DIRECT_BLOCK_NUM {
                // 最后非空块填满了，申请一块新的一级块
                return alloc_new_second(object, inode.get_second_id()).await;
            }
            // 超限
            Err(Error::new(ErrorKind::OutOfMemory, "no valid block"))
        }
    }
}

/// 清空这块block的内容
pub async fn clear_block(block_id: usize) -> Result<(), Error> {
    read_block_to_cache(block_id).await?;
    let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
    let mut bcm = blk.write().await;
    let block = bcm.block_cache.get_mut(&block_id).unwrap();
    block.bytes = [0; BLOCK_SIZE];
    block.modified = true;
    Ok(())
}

async fn alloc_new_second<T: Serialize>(object: &T, second_id: usize) -> Result<(), Error> {
    let new_first_block = alloc_bit(BitmapType::Data).await?;
    alloc_new_first(new_first_block as usize, object).await?;
    try_insert_to_block(&new_first_block, second_id).await?;
    Ok(())
}

async fn alloc_new_first<T: Serialize>(first_id: usize, object: &T) -> Result<(), Error> {
    // 申请一块新块
    let new_block_id = alloc_bit(BitmapType::Data).await?;
    trace!("add a new block {}", new_block_id);
    // 将object 写入新块
    write_block(object, new_block_id as usize, 0).await?;
    // 把新块id附加到一级块
    try_insert_to_block(&new_block_id, first_id).await
}

// 尝试写入该block的空闲位置，失败（空间不足）则返回none
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

/// 获取一个直接块
async fn get_direct_block(id: BlockIDType) -> Result<Vec<u8>, Error> {
    get_block_buffer(id as usize, 0, BLOCK_SIZE).await
}

/// 获取一个一级块所包含的所有直接块
async fn get_first_blocks(
    first_id: BlockIDType,
) -> Result<Vec<(BlockLevel, BlockIDType, Vec<u8>)>, Error> {
    let mut v = Vec::new();
    for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
        let start = i * BLOCK_ADDR_SIZE;
        let end = start + BLOCK_ADDR_SIZE;
        let addr_buff = get_block_buffer(first_id as usize, start, end).await?;
        let direct_id: BlockIDType = deserialize(&addr_buff)?;
        if direct_id == 0 {
            break; // 为空
        }
        let buffer = get_direct_block(direct_id).await?;
        v.push((BlockLevel::FirstIndirect, direct_id, buffer));
    }
    Ok(v)
}

/// 获取一个二级块所包含的所有直接块
async fn get_second_blocks(
    second_id: BlockIDType,
) -> Result<Vec<(BlockLevel, BlockIDType, Vec<u8>)>, Error> {
    let mut v = Vec::new();
    for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
        let start = i * BLOCK_ADDR_SIZE;
        let end = start + BLOCK_ADDR_SIZE;
        let addr_buff = get_block_buffer(second_id as usize, start, end).await?;
        let first_id: BlockIDType = deserialize(&addr_buff)?;
        if first_id == 0 {
            break; // 为空，停止
        }
        let mut buffers = get_first_blocks(first_id).await?;
        for (level, _, _) in &mut buffers {
            *level = BlockLevel::SecondIndirect;
        }
        v.append(&mut buffers);
    }
    Ok(v)
}

/// 获取所有直接块（包含空块，即便地址有效）
pub async fn get_all_blocks(
    inode: &Inode,
) -> Result<Vec<(BlockLevel, BlockIDType, Vec<u8>)>, Error> {
    let mut v = Vec::new();
    // 直接块
    for i in 0..DIRECT_BLOCK_NUM {
        let id = inode.addr[i];
        if id == 0 {
            return Ok(v);
        }
        let buffer = get_direct_block(id).await?;
        v.push((BlockLevel::Direct, id, buffer));
    }

    // 一级
    let first_id = inode.get_first_id() as BlockIDType;
    if first_id == 0 {
        return Ok(v);
    }
    v.append(&mut get_first_blocks(first_id).await?);

    // 二级
    let second_id = inode.get_second_id() as BlockIDType;
    if second_id == 0 {
        return Ok(v);
    }
    v.append(&mut get_second_blocks(second_id).await?);

    Ok(v)
}

/// 获取所有非空块
pub async fn get_all_valid_blocks(
    inode: &Inode,
) -> Result<Vec<(BlockLevel, BlockIDType, Vec<u8>)>, Error> {
    let mut v = get_all_blocks(inode).await?;
    // 保留非空block
    v.retain(|(_, _, block)| !is_empty(block));
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
    for i in 0..BLOCK_SIZE / size {
        let start = i * size;
        let end = start + size;
        let buffer = get_block_buffer(block_id, start, end).await?;
        if *object == deserialize(&buffer)? {
            exist = true;
            // 覆盖该位置
            write_block(&T::default(), block_id, start).await?;
            break;
        }
    }
    if !exist {
        return Err(Error::new(ErrorKind::NotFound, ""));
    }
    //2. 再次序列化，判断是否已空, 如果全空 dealloc
    let block = get_block_buffer(block_id, 0, BLOCK_SIZE).await?;
    if !is_empty(&block) {
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
            let mut first_block: Vec<u8>;
            let mut first_id = 0;
            let mut start = 0; // 记录二级块中的一级块条目偏移量

            // 首先对二级块的每个一级地址所记录的直接块去清除记录
            for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
                start = i * BLOCK_ADDR_SIZE;
                let end = start + BLOCK_ADDR_SIZE;
                first_block = get_block_buffer(second_id, start, end).await?;
                first_id = deserialize(&first_block)?;
                if remove_block_addr_in_first_block(first_id, block_id)
                    .await
                    .is_ok()
                {
                    // 找到并清除了，跳出循环
                    break;
                }
            }
            // 然后检查找到的那个一级块是否空，空了就清掉那个一级块在二级块中的记录
            first_block = get_block_buffer(first_id, 0, BLOCK_SIZE).await?;
            if !is_empty(&first_block) {
                // 那个一级块还有条目，直接返回
                return Ok(());
            }
            // 在二级块中清除一级块记录
            write_block(&0u32, second_id, start).await?;

            // 最后检查二级块 如果二级块空了就把二级块也清空
            let second_block = get_block_buffer(second_id, 0, BLOCK_SIZE).await?;
            if !is_empty(&second_block) {
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
    for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
        let start = i * BLOCK_ADDR_SIZE;
        let end = start + BLOCK_ADDR_SIZE;
        let direct_addr = get_block_buffer(first_id, start, end).await?;
        // 在一级块中找到了这个块的地址，清除
        if direct_addr == serialize(&(block_id as u32))? {
            exist = true;
            write_block(&0u32, first_id, start).await?;
            break;
        }
    }
    if !exist {
        return Err(Error::new(ErrorKind::NotFound, ""));
    }
    let first_block = get_block_buffer(first_id, 0, BLOCK_SIZE).await?;
    if !is_empty(&first_block) {
        return Ok(());
    }
    dealloc_data_bit(first_id).await;
    Ok(())
}

/// 判断block是否是全0
pub fn is_empty(block: &[u8]) -> bool {
    for b in block {
        if *b != 0 {
            return false;
        }
    }
    true
}

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
    let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
    let mut blk_w = blk.write().await;
    blk_w.sync_and_clear_cache().await?;
    drop(blk_w);
    // 重新读取已写入的信息
    let fs = Arc::clone(&SFS);
    let mut w = fs.write().await;
    w.update().await;
    trace!("sync all blocks ok");
    Ok(())
}

pub fn deserialize<'a, T: Deserialize<'a>>(buffer: &'a [u8]) -> Result<T, Error> {
    bincode::deserialize(buffer).map_err(|err| Error::new(ErrorKind::Other, err))
}

pub fn serialize<T: Serialize>(object: &T) -> Result<Vec<u8>, Error> {
    bincode::serialize(object).map_err(|err| Error::new(ErrorKind::Other, err))
}
