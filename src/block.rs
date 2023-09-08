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
    bitmap::alloc_bit,
    simple_fs::{BLOCK_SIZE, FS_FILE_NAME},
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
        info!("block {} already in cache", block_id);
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

/// 将一个object附加到该inode所拥有的最后一个块的末尾，如果达到上限返回None
pub fn append_block<T: Serialize>(object: &T, block_addrs: &mut [u32]) -> Option<()> {
    // 搜索所有直接块
    for i in 0..DIRECT_BLOCK_NUM {
        let direct_id = block_addrs[i] as usize;
        if direct_id == 0 {
            //该直接块还未申请,直接申请一个新块写在开头
            let new_block = alloc_bit(crate::bitmap::BitmapType::Data)?;
            block_addrs[i] = new_block;
            write_block(object, new_block as usize, 0);
            return Some(());
        } else if search_direct_block(direct_id, object).is_some() {
            //已经写入了
            return Some(());
        }
    }

    // 搜索一级块
    let first_id = block_addrs[DIRECT_BLOCK_NUM] as usize;
    if search_first_indirect_block(first_id, object).is_some() {
        return Some(());
    };

    // 搜索二级块
    let second_id = block_addrs[FIRST_INDIRECT_NUM] as usize;
    // 获取每一个一级地址
    let size = BLOCK_ADDR_SIZE;
    for i in 0..BLOCK_SIZE / size {
        let mut start = i * size;
        let mut end = start + size;
        // 直接找二级块最后一个非空一级块的地址
        let buffer = get_block_buffer(second_id, start, end)?;
        let mut empty = true;
        if !empty && i != BLOCK_SIZE / size - 1 {
            // 非空且不是最后一块，继续找
            continue;
        }
        if i != BLOCK_SIZE / size - 1 {
            // 不是最后一块，则要寻找该空块的上一块
            start -= size;
            end -= size
        }
        // 获取最后非空块的地址
        let first_buffer = get_block_buffer(second_id, start, end)?;
        let first_id: u32 = bincode::deserialize(&first_buffer).ok()?;
        // 搜索该一级块
        if search_first_indirect_block(first_id as usize, object).is_some() {
            return Some(());
        }
        //如果最后非空块是二级块的最后一个地址，说明二级块全满了，达到上限
        if i == BLOCK_SIZE / size - 1 {
            return None;
        }
        // 最后非空块填满了，申请一块新的一级块
        let new_first_block = alloc_bit(crate::bitmap::BitmapType::Data)?;
        // 再为新一级块申请一块直接块
        let new_block = alloc_bit(crate::bitmap::BitmapType::Data)?;
        // 将object写入该直接块
        write_block(object, new_block as usize, 0);
        // 将新地址写在新一级块开头
        write_block(&new_block, new_first_block as usize, 0);
        //将新一级地址附加在二级块后面
        write_block(&new_first_block, second_id, start + size);
    }
    Some(())
}

fn search_first_indirect_block<T: Serialize>(first_id: usize, object: &T) -> Option<()> {
    let size = BLOCK_ADDR_SIZE;
    for i in 0..BLOCK_SIZE / size {
        let mut start = i * size;
        let mut end = start + size;
        // 直接找一级块最后一个非空直接块的地址
        let buffer = get_block_buffer(first_id, start, end)?;
        let mut empty = true;
        for b in &buffer {
            if *b != 0 {
                empty = false;
                break;
            }
        }
        if !empty && i != BLOCK_SIZE / size - 1 {
            // 非空且不是最后一块，继续找
            continue;
        }
        if i != BLOCK_SIZE / size - 1 {
            // 不是最后一块，则要寻找该空块的上一块
            start -= size;
            end -= size
        }
        // 获取最后非空块的地址
        let direct_buffer = get_block_buffer(first_id, start, end)?;
        let direct_id: u32 = bincode::deserialize(&direct_buffer).ok()?;
        // 搜索该直接块
        if search_direct_block(direct_id as usize, object).is_some() {
            return Some(());
        }
        //如果最后非空块是一级块的最后一个地址，说明一级块全满了，继续找二级块
        if i == BLOCK_SIZE / size - 1 {
            return None;
        }
        // 最后非空块填满了，申请一块新的
        let new_block = alloc_bit(crate::bitmap::BitmapType::Data)?;
        // 将object写入该直接块
        write_block(object, new_block as usize, 0);
        // 将新地址附加在一级块的内容后面
        write_block(&new_block, first_id, start + size);
    }
    Some(())
}

fn search_direct_block<T: Serialize>(direct_id: usize, object: &T) -> Option<()> {
    let size = size_of::<T>();
    let mut start;
    let mut end;
    // 搜索该块的每一个object
    for i in 0..BLOCK_SIZE / size {
        start = i * size;
        end = start + size;
        // 获得object大小的buffer
        let buffer = get_block_buffer(direct_id, start, end)?;
        let mut empty = true;
        for b in &buffer {
            if *b != 0 {
                empty = false;
                break;
            }
        }
        // 如果直接块可以append，直接写入
        if empty {
            write_block(object, direct_id, start);
            return Some(());
        }
    }
    // block 全满
    None
}

lazy_static! {
    pub static ref BLOCK_CACHE_MANAGER: Mutex<BlockCacheManager> =
        Mutex::new(BlockCacheManager::new());
}

#[allow(unused)]
pub fn cache_msg() {
    let bcm = BLOCK_CACHE_MANAGER.lock();
    println!("\ncache info{:?}\n", bcm.block_cache);
}

/// 清空块缓存，写入磁盘中
pub fn sync_all_block_cache() {
    BLOCK_CACHE_MANAGER.lock().block_cache.clear();
}

/// 缓存自动更新策略,当block drop的时候 自动写入本地文件中
impl Drop for Block {
    fn drop(&mut self) {
        if self.modified {
            if let Ok(file) = OpenOptions::new().write(true).open(FS_FILE_NAME) {
                trace!("drop block{}", self.block_id);
                let offset = self.block_id * BLOCK_SIZE;
                let _ = file
                    .write_all_at(&self.bytes, offset as u64)
                    .map_err(|err| error!("error writing blocks:{}", err));
            }
        }
    }
}
