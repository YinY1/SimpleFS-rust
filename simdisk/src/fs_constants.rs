use std::mem::size_of;

use crate::{block::BlockIDType, dirent::DirEntry, inode::Inode, super_block::SuperBlock};

pub const FS_FILE_NAME: &str = "SIMPLE_FS";

pub const MAGIC: usize = 0x2F02BA345D;

//* 布局 */
pub const BLOCK_SIZE: usize = 1024; // 设块大小为 1KB

pub const FS_SIZE: usize = 100 * 1024 * 1024; // 文件系统大小为 100MB

pub const INODE_BITMAP_NUM: usize = 1; // inode bitmap块数

pub const DATA_BITMAP_NUM: usize = 12; // data bitmap块数

pub const INODE_MAX_NUM: usize = INODE_BITMAP_NUM * BLOCK_SIZE * 8; // inode 总数

pub const INODE_BLOCK_NUM: usize = INODE_MAX_NUM * INODE_SIZE / BLOCK_SIZE; // inode 区块数

pub const DATA_BLOCK_MAX_NUM: usize = DATA_BITMAP_NUM * BLOCK_SIZE * 8; // data块总数

//* 块号分配 */
pub const INODE_BITMAP_START_BLOCK: usize = INODE_BITMAP_NUM; // inode bitmap起始块号

pub const DATA_BITMAP_START_BLOCK: usize = INODE_BITMAP_START_BLOCK + INODE_BITMAP_NUM; // data bitmap起始块号

pub const INODE_START_BLOCK: usize = DATA_BITMAP_START_BLOCK + DATA_BITMAP_NUM; // inode 区起始块号

pub const DATA_START_BLOCK: usize = INODE_START_BLOCK + INODE_BLOCK_NUM; // data 区起始块号

pub const USER_START_BYTE: usize = size_of::<SuperBlock>() + 16; // 用户信息起始位置，加一些偏移防止重叠

//* 寻址 */
pub const DIRECT_BLOCK_NUM: usize = 8; // 直接块数
pub const FIRST_INDIRECT_NUM: usize = 1; // 一级间接块数
pub const SECOND_INDIRECT_NUM: usize = 1; // 二级间接块数
pub const ADDR_TOTAL_SIZE: usize = DIRECT_BLOCK_NUM + FIRST_INDIRECT_NUM + SECOND_INDIRECT_NUM;

pub const BLOCK_ADDR_SIZE: usize = size_of::<BlockIDType>(); // 块地址大小
pub const INODE_SIZE: usize = size_of::<Inode>();
pub const DIRENTRY_SIZE: usize = size_of::<DirEntry>();

pub const INDIRECT_ADDR_NUM: usize = BLOCK_SIZE / BLOCK_ADDR_SIZE; // 间接块可以存下的块地址的数量pub
pub const FISRT_MAX: usize = FIRST_INDIRECT_NUM * INDIRECT_ADDR_NUM; //一级间接块最大可表示的块数量
pub const SECOND_MAX: usize = (SECOND_INDIRECT_NUM * INDIRECT_ADDR_NUM) * FISRT_MAX; //二级间接块最大可表示的块数量

// 文件名和扩展名长度限制（字节）
pub const NAME_LENGTH_LIMIT: usize = 10;
pub const EXTENSION_LENGTH_LIMIT: usize = 3;

pub const MAX_FILE_SIZE: usize = BLOCK_SIZE * (DIRECT_BLOCK_NUM + FISRT_MAX + SECOND_MAX); //可表示文件的最大大小（字节）

pub const SYNC_BLOCK_DURATION: u64 = 60;
