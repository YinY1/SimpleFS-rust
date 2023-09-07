#[allow(unused)]
use log::{debug, error, info, trace};
use spin::Mutex;
use std::{fs::File, io::Write};

use crate::{
    bitmap::{count_data_blocks, count_inodes},
    block::sync_all_block_cache,
    inode::Inode,
    super_block::SuperBlock,
};

pub const FS_FILE_NAME: &str = "SAMPLE_FS";
pub const MAGIC: usize = 0x2F02BA345D;
// 设块大小为 1KB
pub const BLOCK_SIZE: usize = 1024;
// 文件系统大小为 100MB
pub const FS_SIZE: usize = 100 * 1024 * 1024;
// inode bitmap块数
pub const INODE_BITMAP_NUM: usize = 1;
// data bitmap块数
pub const DATA_BITMAP_NUM: usize = 13;
// inode 区块数
pub const INODE_NUM: usize = 1024;
// data 区块数 (<= bitmap bit数,因为系统限制，bitmap有冗余)
pub const DATA_NUM: usize =
    FS_SIZE / BLOCK_SIZE - INODE_NUM - DATA_BITMAP_NUM - INODE_BITMAP_NUM - 1;
// inode bitmap起始块号
pub const INODE_BITMAP_BLOCK: usize = INODE_BITMAP_NUM;
// data bitmap起始块号
pub const DATA_BITMAP_BLOCK: usize = INODE_BITMAP_BLOCK + INODE_BITMAP_NUM;
// inode 区起始块号
pub const INODE_BLOCK: usize = DATA_BITMAP_BLOCK + DATA_BITMAP_NUM;
// data 区起始块号
pub const DATA_BLOCK: usize = INODE_BLOCK + INODE_NUM;

#[allow(unused)]
pub struct SampleFileSystem {
    pub root_inode: Inode,
    pub super_block: SuperBlock,
    pub current_inode: Inode,
}

impl SampleFileSystem {
    /// 从文件系统中读出相关信息
    pub fn read() -> Option<Self> {
        trace!("read SFS");
        let root_inode = Inode::read(0)?;
        Some(Self {
            current_inode: root_inode.clone(),
            root_inode,
            super_block: SuperBlock::read()?,
        })
    }
    ///初始化SFS
    pub fn init() -> Self {
        if let Some(sp) = SuperBlock::read() {
            if sp.valid() {
                trace!("no need to init fs");
                return Self::read().unwrap();
            }
        }

        info!("init fs");
        // 创建100MB空文件
        let mut fs_file = File::create(FS_FILE_NAME).expect("cannot create fs file");
        fs_file
            .write_all(&[0u8; FS_SIZE])
            .expect("cannot init fs file");

        drop(fs_file);

        // 创建超级块
        let super_block = SuperBlock::new();

        // 创建root_inode
        let root_inode = Inode::new_root();

        sync_all_block_cache();
        Self {
            current_inode: root_inode.clone(),
            root_inode,
            super_block,
        }
    }

    pub fn info(&self) {
        println!("-----------------------");
        println!("{:?}", self.super_block);
        println!("{:?}", self.root_inode);
        let (alloced, _) = count_inodes();
        println!("[file counts] {}", alloced);
        let (alloced, valid) = count_data_blocks();
        println!("[Disk used ]  {}", alloced * BLOCK_SIZE,);
        println!("[Disk valid]  {}", valid * BLOCK_SIZE)
    }
}

lazy_static! {
    pub static ref SFS: Mutex<SampleFileSystem> = Mutex::new(SampleFileSystem::init());
}
