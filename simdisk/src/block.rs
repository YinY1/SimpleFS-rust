use log::{error, info, trace};
use serde::{de::DeserializeOwned, Serialize};
use spin::Mutex;
use std::{
    collections::VecDeque,
    fs::{File, OpenOptions},
    io::ErrorKind,
    mem::size_of,
    os::unix::prelude::FileExt,
};

use crate::{
    bitmap::{alloc_bit, dealloc_data_bit, BitmapType},
    inode::Inode,
    simple_fs::{BLOCK_SIZE, FS_FILE_NAME, SFS},
};

pub type BlockIDType = u32;

const BLOCK_CACHE_LIMIT: usize = 1024; // 块缓冲区大小（块数量）

pub const DIRECT_BLOCK_NUM: usize = 8; // 直接块数
pub const FIRST_INDIRECT_NUM: usize = 1; // 一级间接块数
pub const SECOND_INDIRECT_NUM: usize = 1; // 二级间接块数
pub const ADDR_TOTAL_SIZE: usize = DIRECT_BLOCK_NUM + FIRST_INDIRECT_NUM + SECOND_INDIRECT_NUM;

pub const BLOCK_ADDR_SIZE: usize = size_of::<BlockIDType>(); // 块地址大小
pub const INDIRECT_ADDR_NUM: usize = BLOCK_SIZE / BLOCK_ADDR_SIZE; // 间接块可以存下的块地址的数量pub

pub const FISRT_MAX: usize = FIRST_INDIRECT_NUM * INDIRECT_ADDR_NUM; //一级间接块最大可表示的块数量
pub const SECOND_MAX: usize = (SECOND_INDIRECT_NUM * INDIRECT_ADDR_NUM) * FISRT_MAX; //二级间接块最大可表示的块数量
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
}

pub struct BlockCacheManager {
    pub block_cache: VecDeque<Block>,
}

impl BlockCacheManager {
    pub fn new() -> Self {
        Self {
            block_cache: VecDeque::new(),
        }
    }
}
/// 将块读入缓存中
pub fn read_block_to_cache(block_id: usize) {
    let mut block = Block {
        block_id,
        bytes: [0; BLOCK_SIZE],
        modified: false,
    };
    let mut bcm = BLOCK_CACHE_MANAGER.lock();

    if bcm.block_cache.contains(&block) {
        trace!("block {} already in cache", block_id);
        return;
    }

    let offset = block_id * BLOCK_SIZE;
    match File::open(FS_FILE_NAME) {
        Ok(file) => {
            if file.read_exact_at(&mut block.bytes, offset as u64).is_err() {
                error!("cannot read buffer at {}", offset);
                return;
            }
        }
        Err(error) => {
            match error.kind() {
                ErrorKind::NotFound => {
                    trace!("File not found");
                }
                _ => {
                    error!("Error opening file: {}", error);
                }
            };
            return;
        }
    }

    // 时钟算法管理缓存，队头是刚进来的，队尾是后进来的（方便遍历的时候最快找到刚加入缓存的块）
    if bcm.block_cache.len() == BLOCK_CACHE_LIMIT {
        loop {
            let mut block = bcm.block_cache.pop_back().unwrap();
            if block.modified {
                block.modified = false;
                bcm.block_cache.push_front(block);
            } else {
                break;
            }
        }
    }
    bcm.block_cache.push_front(block);
    assert!(bcm.block_cache.len() <= BLOCK_CACHE_LIMIT);
    trace!("block {} push to cache", block_id);
}

/// 获取指定块中的某一段缓存
pub fn get_block_buffer(block_id: usize, start_byte: usize, end_byte: usize) -> Option<Vec<u8>> {
    // 当块不在缓存中时 读入缓存
    read_block_to_cache(block_id);

    let bcm = BLOCK_CACHE_MANAGER.lock();
    for block in &bcm.block_cache {
        if block.block_id == block_id {
            return Some(block.bytes[start_byte..end_byte].to_vec());
        }
    }
    None
}

/// 将`object`序列化并写入指定的`block_id`中，
/// 用`start_byte`指示出该`object`会在块中的字节起始位置
pub fn write_block<T: serde::Serialize>(object: &T, block_id: usize, start_byte: usize) {
    trace!("write block{}", block_id);
    // 当块不在缓存中时 读入缓存
    read_block_to_cache(block_id);

    let mut bcm = BLOCK_CACHE_MANAGER.lock();
    for block in &mut bcm.block_cache {
        if block.block_id == block_id {
            // 将 object 序列化
            match bincode::serialize(object) {
                Ok(obj_bytes) => {
                    let end_byte = obj_bytes.len() + start_byte;
                    trace!("write block{}, len {}B", block_id, obj_bytes.len());
                    block.modify_bytes(|bytes_arr| {
                        bytes_arr[start_byte..end_byte].clone_from_slice(&obj_bytes);
                    });
                    return;
                }
                Err(err) => {
                    error!("cannot serialize:{}", err)
                }
            }
        }
    }
    error!("unreachable write_block");
}

/// 尝试插入一个object到磁盘中
pub fn insert_object<T: Serialize + Default + DeserializeOwned + PartialEq>(
    object: &T,
    inode: &mut Inode,
) -> Option<()> {
    let all_blocks = get_all_blocks(inode)?;
    for (_, id, _) in &all_blocks {
        if try_insert_to_block(object, *id as usize).is_some() {
            return Some(());
        }
        // 如果该块没有空余，继续找
    }
    // 没有空余的，申请
    let last_level = &all_blocks.last()?.0;
    match *last_level {
        BlockLevel::Direct => {
            //申请一个块
            for i in 0..DIRECT_BLOCK_NUM {
                if inode.addr[i] == 0 {
                    let new_block_id = alloc_bit(BitmapType::Data)?;
                    trace!("add a new direct block {}", new_block_id);
                    // 将地址写回inode中
                    inode.addr[i] = new_block_id;
                    write_block(object, new_block_id as usize, 0);
                    return Some(());
                }
            }
            // 直接块用完了，要申请一个新的一级块
            let new_first_id = alloc_bit(BitmapType::Data)?;
            trace!("add a new first block {}", new_first_id);
            // 将一级地址写回inode中
            inode.set_first_id(new_first_id);
            return alloc_new_first(new_first_id as usize, object);
        }
        BlockLevel::FirstIndirect => {
            // 一级间接块的已有的所有直接块没有空间了
            if all_blocks.len() < FISRT_MAX + DIRECT_BLOCK_NUM {
                // 一级间接块本身还有空间，直接附加
                return alloc_new_first(inode.get_first_id(), object);
            } else {
                // 一级块没空间了，要找二级块（返回的是最后一块一级块）
                // 申请一块新的二级块
                let new_second_id = alloc_bit(BitmapType::Data)?;
                // 将二级地址写回inode中
                inode.set_second_id(new_second_id);
                return alloc_new_second(object, new_second_id as usize);
            }
        }
        BlockLevel::SecondIndirect => {
            if all_blocks.len() < SECOND_MAX + FISRT_MAX + DIRECT_BLOCK_NUM {
                // 最后非空块填满了，申请一块新的一级块
                return alloc_new_second(object, inode.get_second_id());
            }
            // 超限
        }
    }
    None
}

pub fn clear_block(block_id: usize) {
    read_block_to_cache(block_id);

    let mut bcm = BLOCK_CACHE_MANAGER.lock();
    for block in &mut bcm.block_cache {
        if block.block_id == block_id {
            block.bytes = [0; BLOCK_SIZE];
            block.modified = true;
            return;
        }
    }
    error!("unreachable clear_block");
}

fn alloc_new_second<T: Serialize>(object: &T, second_id: usize) -> Option<()> {
    let new_first_block = alloc_bit(BitmapType::Data)?;
    alloc_new_first(new_first_block as usize, object)?;
    try_insert_to_block(&new_first_block, second_id)?;
    Some(())
}

fn alloc_new_first<T: Serialize>(first_id: usize, object: &T) -> Option<()> {
    // 申请一块新块
    let new_block_id = alloc_bit(BitmapType::Data)?;
    trace!("add a new block {}", new_block_id);
    // 将object 写入新块
    write_block(object, new_block_id as usize, 0);
    // 把新块id附加到一级块
    try_insert_to_block(&new_block_id, first_id)
}

// 尝试写入该block的空闲位置，失败（空间不足）则返回none
fn try_insert_to_block<T: Serialize + Default + DeserializeOwned + PartialEq>(
    object: &T,
    block_id: usize,
) -> Option<()> {
    let size = size_of::<T>();
    // 搜索该块的每一个object
    for i in 0..BLOCK_SIZE / size {
        let start = i * size;
        let end = start + size;
        // 获得object大小的buffer
        let buffer = get_block_buffer(block_id, start, end)?;
        // 如果是默认值（空余位置）
        let obj: T = bincode::deserialize(&buffer).ok()?;
        if obj == T::default() {
            write_block(object, block_id, start);
            return Some(());
        }
    }
    // block 没有足够空间
    None
}

/// 获取一个直接块
fn get_direct_block(id: BlockIDType) -> Option<Vec<u8>> {
    get_block_buffer(id as usize, 0, BLOCK_SIZE)
}

/// 获取一个一级块所包含的所有直接块
fn get_first_blocks(first_id: BlockIDType) -> Option<Vec<(BlockLevel, BlockIDType, Vec<u8>)>> {
    let mut v = Vec::new();
    for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
        let start = i * BLOCK_ADDR_SIZE;
        let end = start + BLOCK_ADDR_SIZE;
        let addr_buff = get_block_buffer(first_id as usize, start, end)?;
        let direct_id: BlockIDType = bincode::deserialize(&addr_buff).ok()?;
        let buffer = get_direct_block(direct_id)?;
        v.push((BlockLevel::FirstIndirect, direct_id, buffer));
    }
    Some(v)
}

/// 获取一个二级块所包含的所有直接块
fn get_second_blocks(second_id: BlockIDType) -> Option<Vec<(BlockLevel, BlockIDType, Vec<u8>)>> {
    let mut v = Vec::new();
    for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
        let start = i * BLOCK_ADDR_SIZE;
        let end = start + BLOCK_ADDR_SIZE;
        let addr_buff = get_block_buffer(second_id as usize, start, end)?;
        let first_id: BlockIDType = bincode::deserialize(&addr_buff).ok()?;
        let mut buffers = get_first_blocks(first_id)?;
        for (level, _, _) in &mut buffers {
            *level = BlockLevel::SecondIndirect;
        }
        v.append(&mut buffers);
    }
    Some(v)
}

/// 获取所有直接块（包含空块，即便地址有效）
pub fn get_all_blocks(inode: &Inode) -> Option<Vec<(BlockLevel, BlockIDType, Vec<u8>)>> {
    let mut v = Vec::new();
    // 直接块
    for i in 0..DIRECT_BLOCK_NUM {
        let id = inode.addr[i];
        if id == 0 {
            return Some(v);
        }
        let buffer = get_direct_block(id)?;
        v.push((BlockLevel::Direct, id, buffer));
    }

    // 一级
    let first_id = inode.get_first_id() as BlockIDType;
    if first_id == 0 {
        return Some(v);
    }
    v.append(&mut get_first_blocks(first_id)?);

    // 二级
    let second_id = inode.get_second_id() as BlockIDType;
    if second_id == 0 {
        return Some(v);
    }
    v.append(&mut get_second_blocks(second_id)?);

    Some(v)
}

/// 获取所有非空块
pub fn get_all_valid_blocks(inode: &Inode) -> Option<Vec<(BlockLevel, BlockIDType, Vec<u8>)>> {
    let mut v = get_all_blocks(inode)?;
    // 保留非空block
    v.retain(|(_, _, block)| !is_empty(block));
    Some(v)
}

/// 移除一个object，如果这是唯一的object，那么释放这个block
pub fn remove_object<T: Serialize + Default + PartialEq + DeserializeOwned>(
    object: &T,
    block_id: usize,
    level: BlockLevel,
    inode: &mut Inode,
) -> Option<()> {
    //1.序列化这个block，一一比较
    let size = size_of::<T>();
    let mut exist = false;
    for i in 0..BLOCK_SIZE / size {
        let start = i * size;
        let end = start + size;
        let buffer = get_block_buffer(block_id, start, end)?;
        if *object == bincode::deserialize(&buffer).ok()? {
            exist = true;
            // 覆盖该位置
            write_block(&T::default(), block_id, start);
            break;
        }
    }
    if !exist {
        return None;
    }
    //2. 再次序列化，判断是否已空, 如果全空 dealloc
    let block = get_block_buffer(block_id, 0, BLOCK_SIZE)?;
    if !is_empty(&block) {
        return Some(());
    }
    dealloc_data_bit(block_id);

    match level {
        BlockLevel::Direct => {
            //3.1. 如果是直接块，去inode将地址置空
            for i in 0..DIRECT_BLOCK_NUM {
                if block_id == inode.addr[i] as usize {
                    inode.addr[i] = 0;
                    return Some(());
                }
            }
            panic!("unreachable");
        }
        BlockLevel::FirstIndirect => {
            //3.2. 如果是在一级块，那么还要清除在一级块中的地址，判断释放这个block addr之后一级块是否已空
            let first_id = inode.get_first_id();
            // 在一级块中清除该块的地址
            remove_block_addr_in_first_block(first_id, block_id)?;
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
                first_block = get_block_buffer(second_id, start, end)?;
                first_id = bincode::deserialize(&first_block).ok()?;
                if remove_block_addr_in_first_block(first_id, block_id).is_some() {
                    // 找到并清除了，跳出循环
                    break;
                }
            }
            // 然后检查找到的那个一级块是否空，空了就清掉那个一级块在二级块中的记录
            first_block = get_block_buffer(first_id, 0, BLOCK_SIZE)?;
            if !is_empty(&first_block) {
                // 那个一级块还有条目，直接返回
                return Some(());
            }
            // 在二级块中清除一级块记录
            write_block(&0u32, second_id, start);

            // 最后检查二级块 如果二级块空了就把二级块也清空
            let second_block = get_block_buffer(second_id, 0, BLOCK_SIZE)?;
            if !is_empty(&second_block) {
                return Some(());
            }
            // 全空, 释放二级块
            dealloc_data_bit(second_id);
            inode.set_second_id(0);
        }
    }
    Some(())
}

/// 清除一级块中的直接块地址条目，同时一级块变空时dealloc一级块
fn remove_block_addr_in_first_block(first_id: usize, block_id: usize) -> Option<()> {
    let mut exist = false;
    for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
        let start = i * BLOCK_ADDR_SIZE;
        let end = start + BLOCK_ADDR_SIZE;
        let direct_addr = get_block_buffer(first_id, start, end)?;
        // 在一级块中找到了这个块的地址，清除
        if direct_addr == bincode::serialize(&(block_id as u32)).ok()? {
            exist = true;
            write_block(&0u32, first_id, start);
            break;
        }
    }
    if !exist {
        return None;
    }
    let first_block = get_block_buffer(first_id, 0, BLOCK_SIZE)?;
    if !is_empty(&first_block) {
        return Some(());
    }
    dealloc_data_bit(first_id);
    Some(())
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
    pub static ref BLOCK_CACHE_MANAGER: Mutex<BlockCacheManager> =
        Mutex::new(BlockCacheManager::new());
}

#[derive(Clone, Copy)]
pub enum BlockLevel {
    Direct,
    FirstIndirect,
    SecondIndirect,
}

#[allow(unused)]
pub fn cache_msg() {
    let bcm = BLOCK_CACHE_MANAGER.lock();
    println!("\ncache info{:?}\n", bcm.block_cache);
}

/// 清空块缓存，写入磁盘中
pub fn sync_all_block_cache() {
    BLOCK_CACHE_MANAGER.lock().block_cache.clear();
    // 重新读取已写入的信息
    SFS.lock().update();
}

/// 缓存自动更新策略,当block drop的时候 自动写入本地文件中
impl Drop for Block {
    fn drop(&mut self) {
        if self.modified {
            if let Ok(file) = OpenOptions::new().write(true).open(FS_FILE_NAME) {
                info!("drop block{}", self.block_id);
                let offset = self.block_id * BLOCK_SIZE;
                let _ = file
                    .write_all_at(&self.bytes, offset as u64)
                    .map_err(|err| error!("error writing blocks:{}", err));
            }
        }
    }
}
