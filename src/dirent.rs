use log::error;
use serde::{Deserialize, Serialize};

use crate::{
    block::{write_block, append_block},
    inode::{FileMode, Inode, InodeType},
};

// 文件名和扩展名长度限制（字节）
const NAME_LENGTH_LIMIT: usize = 10;
const EXTENSION_LENGTH_LIMIT: usize = 3;

#[allow(unused)]
#[derive(Serialize, Deserialize, Debug)]
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

    /// 在给定inode下生成一个子目录，
    pub fn new(filename: &str, extension: &str, inode: &mut Inode) -> Option<Self> {
        if filename.len() > NAME_LENGTH_LIMIT {
            error!("filename TOO LONG");
            None
        } else if extension.len() > EXTENSION_LENGTH_LIMIT {
            error!("extension TOO LONG");
            None
        } else {
            let mut filename_ = [0; NAME_LENGTH_LIMIT];
            filename_[..filename.len()].copy_from_slice(filename.as_bytes());
            let mut extension_ = [0; EXTENSION_LENGTH_LIMIT];
            extension_[..extension.len()].copy_from_slice(extension.as_bytes());

            Some(Self {
                filename: filename_,
                extension: extension_,
                inode_id: inode.inode_id,
            })
        }
    }

    pub fn create_dot(inode: &mut Inode) -> Self {
        let dirent = Self::new(".", "", inode).unwrap();
        inode.linkat();
        dirent
    }

    pub fn create_dot_dot(inode: &mut Inode) -> Self {
        let dirent = Self::new("..", "", inode).unwrap();
        inode.linkat();
        dirent
    }

    pub fn create_diretory(current_inode: &mut Inode, parent_inode: &mut Inode) -> (Self, Self) {
        (
            Self::create_dot(current_inode),
            Self::create_dot_dot(parent_inode),
        )
    }

    pub fn get_filename(&self) -> String {
        let name = String::from_utf8_lossy(&self.filename)
            .split('\0')
            .next()
            .unwrap()
            .to_string();
        let ext = String::from_utf8_lossy(&self.extension)
            .split('\0')
            .next()
            .unwrap()
            .to_string();
        if !ext.is_empty() {
            name + "." + &ext
        } else {
            name
        }
    }
}

pub fn mkdir(name: &str, parent_inode: &mut Inode) -> Option<()> {
    // 生成一个名为name的dirent存在父节点的block中
    let (filename, ext) = match name.rsplit_once('.') {
        Some(it) => it,
        None => (name, ""),
    };
    let dirent = DirEntry::new(filename, ext, parent_inode)?;
    append_block(&dirent,&mut parent_inode.addr)?;
    Inode::alloc_dir(parent_inode)?;
    Some(())
}
