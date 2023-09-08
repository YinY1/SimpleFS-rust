use log::{error, info, trace};
use serde::Serialize;
use spin::Mutex;
use std::{
    collections::VecDeque,
    fs::{File, OpenOptions},
    io::ErrorKind,
    mem::size_of,
    os::unix::prelude::FileExt,
};

use crate::{
    bitmap::{alloc_bit, BitmapType},
    simple_fs::{BLOCK_SIZE, FS_FILE_NAME, SFS},
};

const BLOCK_CACHE_LIMIT: usize = 1024; // 块缓冲区大小（块数量）

pub const DIRECT_BLOCK_NUM: usize = 8; // 直接块数
pub const FIRST_INDIRECT_NUM: usize = 1; // 一级间接块数
pub const SECOND_INDIRECT_NUM: usize = 1; // 二级间接块数
pub const ADDR_TOTAL_SIZE: usize = DIRECT_BLOCK_NUM + FIRST_INDIRECT_NUM + SECOND_INDIRECT_NUM;

pub const BLOCK_ADDR_SIZE: usize = size_of::<u32>(); // 块地址大小
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

    // 时钟算法管理缓存
    if bcm.block_cache.len() == BLOCK_CACHE_LIMIT {
        loop {
            let mut blk = bcm.block_cache.pop_front().unwrap();
            if blk.modified {
                blk.modified = false;
                bcm.block_cache.push_back(blk);
            } else {
                break;
            }
        }
    }
    bcm.block_cache.push_back(block);
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
                Ok(bytes) => {
                    let end_byte = bytes.len() + start_byte;
                    trace!("write block{}, len {}B", block_id, bytes.len());
                    block.bytes[start_byte..end_byte].clone_from_slice(&bytes);
                    block.modified = true;
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
pub fn insert_object<T: Serialize>(object: &T, block_addrs: &mut [u32]) -> Option<()> {
    let all_blocks = get_all_blocks(block_addrs)?;
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
                if block_addrs[i] == 0 {
                    let new_block_id = alloc_bit(BitmapType::Data)?;
                    trace!("add a new direct block {}", new_block_id);
                    // 将地址写回inode中
                    block_addrs[i] = new_block_id;
                    write_block(object, new_block_id as usize, 0);
                    return Some(());
                }
            }
            // 直接块用完了，要申请一个新的一级块
            let new_first_id = alloc_bit(BitmapType::Data)?;
            trace!("add a new first block {}", new_first_id);
            // 将一级地址写回inode中
            block_addrs[DIRECT_BLOCK_NUM] = new_first_id;
            return alloc_new_first(new_first_id as usize, object);
        }
        BlockLevel::FirstIndirect => {
            // 一级间接块的已有的所有直接块没有空间了
            if all_blocks.len() < FISRT_MAX + DIRECT_BLOCK_NUM {
                // 一级间接块本身还有空间，直接附加
                return alloc_new_first(block_addrs[DIRECT_BLOCK_NUM] as usize, object);
            } else {
                // 一级块没空间了，要找二级块（返回的是最后一块一级块）
                // 申请一块新的二级块
                let new_second_id = alloc_bit(BitmapType::Data)?;
                // 将二级地址写回inode中
                block_addrs[DIRECT_BLOCK_NUM + FIRST_INDIRECT_NUM] = new_second_id;
                return alloc_new_second(object, new_second_id as usize);
            }
        }
        BlockLevel::SecondIndirect => {
            if all_blocks.len() < SECOND_MAX + FISRT_MAX + DIRECT_BLOCK_NUM {
                // 最后非空块填满了，申请一块新的一级块
                return alloc_new_second(
                    object,
                    block_addrs[DIRECT_BLOCK_NUM + FIRST_INDIRECT_NUM] as usize,
                );
            }
            // 超限
        }
    }
    None
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
fn try_insert_to_block<T: Serialize>(object: &T, block_id: usize) -> Option<()> {
    let size = size_of::<T>();
    // 搜索该块的每一个object
    for i in 0..BLOCK_SIZE / size {
        let start = i * size;
        let end = start + size;
        // 获得object大小的buffer
        let buffer = get_block_buffer(block_id, start, end)?;
        let mut empty = true;
        for b in &buffer {
            if *b != 0 {
                empty = false;
                break;
            }
        }
        // 如果有空余位置，直接写入
        if empty {
            write_block(object, block_id, start);
            return Some(());
        }
    }
    // block 没有足够空间
    None
}

/// 获取一个直接块
fn get_direct_block(id: u32) -> Option<Vec<u8>> {
    get_block_buffer(id as usize, 0, BLOCK_SIZE)
}

/// 获取一个一级块所包含的所有直接块
fn get_first_blocks(first_id: u32) -> Option<Vec<(BlockLevel, u32, Vec<u8>)>> {
    let mut v = Vec::new();
    for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
        let start = i * BLOCK_ADDR_SIZE;
        let end = start + BLOCK_ADDR_SIZE;
        let addr_buff = get_block_buffer(first_id as usize, start, end)?;
        let direct_id: u32 = bincode::deserialize(&addr_buff).ok()?;
        let buffer = get_direct_block(direct_id)?;
        v.push((BlockLevel::FirstIndirect, direct_id, buffer));
    }
    Some(v)
}

/// 获取一个二级块所包含的所有直接块
fn get_second_blocks(second_id: u32) -> Option<Vec<(BlockLevel, u32, Vec<u8>)>> {
    let mut v = Vec::new();
    for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
        let start = i * BLOCK_ADDR_SIZE;
        let end = start + BLOCK_ADDR_SIZE;
        let addr_buff = get_block_buffer(second_id as usize, start, end)?;
        let first_id: u32 = bincode::deserialize(&addr_buff).ok()?;
        let mut buffers = get_first_blocks(first_id)?;
        for (level, _, _) in &mut buffers {
            *level = BlockLevel::SecondIndirect;
        }
        v.append(&mut buffers);
    }
    Some(v)
}

/// 获取所有直接块（包含空块，即便地址有效）
pub fn get_all_blocks(block_addrs: &[u32]) -> Option<Vec<(BlockLevel, u32, Vec<u8>)>> {
    let mut v = Vec::new();
    // 直接块
    for i in 0..DIRECT_BLOCK_NUM {
        let id = block_addrs[i];
        if id == 0 {
            return Some(v);
        }
        let buffer = get_direct_block(id)?;
        v.push((BlockLevel::Direct, id, buffer));
    }

    // 一级
    let first_id = block_addrs[DIRECT_BLOCK_NUM];
    if first_id == 0 {
        return Some(v);
    }
    v.append(&mut get_first_blocks(first_id)?);

    // 二级
    let second_id = block_addrs[FIRST_INDIRECT_NUM];
    if second_id == 0 {
        return Some(v);
    }
    v.append(&mut get_second_blocks(second_id)?);

    Some(v)
}

/// 获取所有非空块
pub fn get_all_valid_blocks(block_addrs: &[u32]) -> Option<Vec<(BlockLevel, u32, Vec<u8>)>> {
    let mut v = get_all_blocks(block_addrs)?;
    // 保留非空block
    v.retain(|(_, _, block)| !is_empty(block));
    Some(v)
}

/// 移除一个object，如果这是唯一的object，那么释放这个block
pub fn remove_object<T: Serialize>(object: &T, block_id: u32) {
    todo!()
    //1.序列化这个block，一一比较

    //2. 再次序列化，判断是否已空, 如果全空 dealloc // TODO确保object的new empty方法序列化之后是全0

    //3.1. 如果是直接块，去inode将地址置空

    //3.2. 如果是在一级块，那么还要判断释放这个block之后一级块是否已空

    //3.3. 如果是二级块，判断二级块是否已空
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
    SFS.lock().read();
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
