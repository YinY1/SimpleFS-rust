use log::{error, info, trace};
use spin::Mutex;
use std::{
    collections::VecDeque,
    fs::{File, OpenOptions},
    io::ErrorKind,
    os::unix::prelude::FileExt,
};

use crate::simple_fs::{BLOCK_SIZE, FS_FILE_NAME};

const BLOCK_CACHE_LIMIT: usize = 1024; // 块缓冲区大小（块数量）

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
