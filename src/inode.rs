use bitflags::bitflags;
use log::{error, trace};
use serde::{Deserialize, Serialize};
use std::mem::size_of;

use crate::{
    block::{get_block_buffer, write_block},
    simple_fs::{BLOCK_SIZE, INODE_BLOCK},
};

pub const INODE_SIZE: usize = size_of::<Inode>();
pub const DIRENTRY_SIZE: usize = size_of::<DirEntry>();
const NAME_LENGTH_LIMIT: usize = 10;
const EXTENSION_LENGTH_LIMIT: usize = 3;

#[derive(Serialize, Deserialize, Debug)]
pub struct Inode {
    inode_type: InodeType,
    mode: FileMode,    // 权限
    pub inode_id: u16, // inode 号
    nlink: u8,
    uid: u32,
    gid: u32,
    size: u32,
    addr: [u32; 11], // 9个直接，1个一级，一个2级，最大33MB
}

#[derive(Serialize, Deserialize, Debug)]

pub enum InodeType {
    File,
    Diretory,
}

bitflags! {
    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
    #[serde(transparent)]
    pub struct FileMode:u8{
         /// 只读
         const RDONLY = 1 << 0;
         /// 只写
         const WRONLY = 1 << 1;
         /// 读写
         const RDWR = 1 << 2;
         /// 可执行
         const EXCUTE = 1 << 3;
    }
}

impl Inode {
    pub fn new(inode_type: InodeType, inode_id: u16, mode: FileMode) -> Self {
        Self {
            inode_type,
            mode,
            inode_id,
            nlink: 1,
            uid: 0,
            gid: 0,
            size: 0,
            addr: [0u32; 11],
        }
    }

    pub fn read(inode_id: usize) -> Option<Self> {
        let block_id = inode_id / BLOCK_SIZE + INODE_BLOCK;
        let inode_pos = inode_id % 16;
        let start_byte = inode_pos * INODE_SIZE;
        let end_byte = start_byte + INODE_SIZE;

        // 一个Inode 64B
        let buffer = get_block_buffer(block_id, start_byte, end_byte)?;
        bincode::deserialize(&buffer).ok()
    }

    pub fn write(&self) {
        let inode_id = self.inode_id as usize;
        let block_id = inode_id / BLOCK_SIZE + INODE_BLOCK;
        let inode_pos = inode_id % 16;
        let start_byte = inode_pos * INODE_SIZE;
        let end_byte = start_byte + INODE_SIZE;

        trace!("write inode {} to block {} cache\n", inode_id, block_id);
        write_block(self, block_id, start_byte, end_byte);
    }
}

#[allow(unused)]
pub struct DirEntry {
    filename: [u8; NAME_LENGTH_LIMIT],       //文件名：10B
    extension: [u8; EXTENSION_LENGTH_LIMIT], //扩展名: 3B
    inode_id: u16,                           //inode号: 2B
}

#[allow(unused)]
impl DirEntry {
    pub fn new_empty() -> Self {
        Self {
            filename: [0; NAME_LENGTH_LIMIT],
            extension: [0; EXTENSION_LENGTH_LIMIT],
            inode_id: 0,
        }
    }

    pub fn new(filename: &str, extension: &str, inode_id: u16) -> Option<Self> {
        if filename.len() > NAME_LENGTH_LIMIT {
            error!("filename TOO LONG");
            None
        } else if extension.len() > EXTENSION_LENGTH_LIMIT {
            error!("extension TOO LONG");
            None
        } else {
            let mut filename_ = [0; NAME_LENGTH_LIMIT];
            filename_.copy_from_slice(filename.as_bytes());
            let mut extension_ = [0; EXTENSION_LENGTH_LIMIT];
            extension_.copy_from_slice(extension.as_bytes());
            Some(Self {
                filename: filename_,
                extension: extension_,
                inode_id,
            })
        }
    }
}
