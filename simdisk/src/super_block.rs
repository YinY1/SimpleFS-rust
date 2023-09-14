use crate::{
    block::{deserialize, get_block_buffer, write_block},
    simple_fs::*,
};
use log::trace;
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, io::Error};

/// 共100K块，SB一块
///
/// inode bitmap一块，共1*1K*8=8K位，表示8K个文件
///
/// data bitmap 13块，共13*1K*8*1K=104M,
///
/// inode区 1K块，每个inode 64B，共1K*1K/64=8K个文件
///
/// 剩下的都是data区块
#[derive(Serialize, Deserialize, Debug, Default)]
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

#[allow(unused)]
impl SuperBlock {
    /// 初始化超级块
    pub async fn new() -> Self {
        trace!("init super block");
        let sb = Self {
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
        };
        sb.cache().await;
        sb
    }

    pub async fn cache(&self) {
        trace!("write super block to cache");
        write_block(self, 0, 0).await;
    }

    pub async fn read() -> Result<Self, Error> {
        trace!("read super block cache");
        let buffer = get_block_buffer(0, 0, BLOCK_SIZE).await?;
        deserialize(&buffer)
    }

    pub fn valid(&self) -> bool {
        self.magic == MAGIC
    }
}
