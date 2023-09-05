#[allow(unused)]
use log::{debug, error, info, trace};
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Debug, Formatter, Result},
    fs::{File, OpenOptions},
    io::Write,
    os::unix::prelude::FileExt,
};

const FS_FILE_NAME: &str = "SAMPLE_FS";
const MAGIC: usize = 0x2F02BA345D;
// 设块大小为 1KB
const BLOCK_SIZE: usize = 1024;
// 文件系统大小为 100MB
const FS_SIZE: usize = 100 * 1024 * 1024;
// inode bitmap块数
const INODE_BITMAP_NUM: usize = 1;
// data bitmap块数
const DATA_BITMAP_NUM: usize = 13;
// inode 区块数
const INODE_NUM: usize = 1024;
// inode bitmap起始块号
const INODE_BITMAP_BLOCK: usize = INODE_BITMAP_NUM;
// data bitmap起始块号
const DATA_BITMAP_BLOCK: usize = INODE_BITMAP_BLOCK + INODE_BITMAP_NUM;
// inode 区起始块号
const INODE_BLOCK: usize = DATA_BITMAP_BLOCK + DATA_BITMAP_NUM;
// data 区起始块号
const DATA_BLOCK: usize = INODE_BLOCK + INODE_NUM;

/// 共100K块，SB一块
///
/// inode bitmap一块，共1*1K*8=8K位，表示8K个文件
///
/// data bitmap 13块，共13*1K*8*1K=104M,
///
/// inode区 1K块，每个inode 64B，共1K*1K/64=8K个文件
///
/// 剩下的都是data区块
#[derive(Serialize, Deserialize)]
pub struct SuperBlock {
    magic: usize,   //魔数
    fs_size: usize, // 文件系统大小，块为单位

    // inode info
    first_block_of_inode_bitmap: usize, // inode位图区起始块号
    inode_bitmap_size: usize,           // inode位图区大小，块为单位
    first_inode: usize,                 // inode区起始块号
    inode_area_size: usize,             // inode区大小 ，块为单位

    // data info
    first_block_of_data_bitmap: usize, // 数据块位图 起始块号
    data_bitmap_size: usize,           // 数据块位图大小 ，块为单位
    first_data_block: usize,           // 数据区第一块的块号，放置根目录
    data_size: usize,                  // 数据区大小，块为单位
}

impl Debug for SuperBlock {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_struct("SuperBlock")
            .field("fs_size", &self.fs_size)
            .field("first_block", &self.first_data_block)
            .field("first_inode", &self.first_inode)
            .field("inode_area_size", &self.inode_area_size)
            .field(
                "first_block_of_inode_bitmap",
                &self.first_block_of_inode_bitmap,
            )
            .field("inode_bitmap_size", &self.inode_bitmap_size)
            .field("data_size", &self.data_size)
            .field(
                "first_block_of_data_bitmap",
                &self.first_block_of_data_bitmap,
            )
            .field("data_bitmap_size", &self.data_bitmap_size)
            .finish()
    }
}

#[allow(unused)]
impl SuperBlock {
    /// 初始化超级块
    pub fn new() -> Self {
        Self {
            fs_size: FS_SIZE / BLOCK_SIZE,
            first_inode: INODE_BLOCK,
            inode_area_size: INODE_NUM,
            first_block_of_inode_bitmap: INODE_BITMAP_BLOCK,
            inode_bitmap_size: INODE_BITMAP_NUM,
            data_size: FS_SIZE - DATA_BLOCK,
            first_data_block: DATA_BLOCK,
            first_block_of_data_bitmap: DATA_BITMAP_BLOCK,
            data_bitmap_size: DATA_BITMAP_NUM,
            magic: MAGIC,
        }
    }

    pub fn write(&self) {
        write_blocks(self, 0);
    }

    pub fn read() -> Option<Self> {
        let mut buffer = vec![0u8; BLOCK_SIZE];
        read_blocks(&mut buffer, 0)
    }
}

pub fn init() {
    if let Some(sp) = SuperBlock::read() {
        if sp.magic == MAGIC {
            trace!("no need to init fs");
            return;
        }
    }
    info!("init fs");
    // 创建100MB空文件
    let mut fs_file = File::create(FS_FILE_NAME).expect("cannot create fs file");
    fs_file
        .write_all(&[0u8; FS_SIZE])
        .expect("cannot init fs file");
    SuperBlock::new().write();
}

pub fn write_blocks<T: serde::Serialize>(object: &T, offset: u64) {
    match bincode::serialize(object) {
        Ok(bytes) => {
            if let Ok(file) = OpenOptions::new().write(true).open(FS_FILE_NAME) {
                let _ = file
                    .write_all_at(&bytes, offset)
                    .map_err(|err| error!("error writing blocks:{}", err));
            } else {
                error!("cannot open {}", FS_FILE_NAME);
            }
        }
        Err(err) => {
            error!("cannot serialize:{}", err)
        }
    }
}

pub fn read_blocks<'a, T: serde::Deserialize<'a>>(buffer: &'a mut [u8], offset: u64) -> Option<T> {
    if let Ok(file) = File::open(FS_FILE_NAME) {
        if file.read_exact_at(buffer, offset).is_err() {
            error!("cannot read buffer at {}", offset);
            None
        } else {
            bincode::deserialize(buffer).ok()
        }
    } else {
        error!("cannot open fs file");
        None
    }
}
