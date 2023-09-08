use log::error;
use serde::{Deserialize, Serialize};

use crate::{
    block::{get_all_valid_blocks, get_block_buffer, insert_object},
    inode::{Inode, DIRENTRY_SIZE},
    simple_fs::BLOCK_SIZE,
};

// 文件名和扩展名长度限制（字节）
const NAME_LENGTH_LIMIT: usize = 10;
const EXTENSION_LENGTH_LIMIT: usize = 3;

#[allow(unused)]
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
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

    /// 返回一个dirent数组，以及所在的block
    pub fn get_all_dirent(block_addrs: &[u32]) -> Option<Vec<(u32, Self)>> {
        let mut dirs = Vec::new();
        for (_, block_id, _) in &get_all_valid_blocks(block_addrs)? {
            if *block_id == 0 {
                break;
            }
            for i in 0..BLOCK_SIZE / DIRENTRY_SIZE {
                let start = i * DIRENTRY_SIZE;
                let end = start + DIRENTRY_SIZE;
                let buffer = get_block_buffer(*block_id as usize, start, end)?;
                // 名字第一个字节为空 说明不是dirent
                if buffer[0] == 0 {
                    break;
                }
                let dir: DirEntry = bincode::deserialize(&buffer).ok()?;
                dirs.push((*block_id, dir));
            }
        }
        Some(dirs)
    }

    /// 如果dirent存在，返回所在block id，否则none
    pub fn get_block_id(&self, block_addrs: &[u32]) -> Option<u32> {
        Self::get_all_dirent(block_addrs)
            .unwrap()
            .iter()
            .find_map(
                |(block_id, dir)| {
                    if self == dir {
                        Some(*block_id)
                    } else {
                        None
                    }
                },
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
    if dirent.get_block_id(&parent_inode.addr).is_some() {
        println!("diretory {} already exist", name);
        return None;
    }
    insert_object(&dirent, &mut parent_inode.addr)?;
    Inode::alloc_dir(parent_inode)?;
    Some(())
}

pub fn rmdir(name: &str, parent_inode: &mut Inode) -> Option<()> {
    let (filename, ext) = match name.rsplit_once('.') {
        Some(it) => it,
        None => (name, ""),
    };
    let dirent = DirEntry::new(filename, ext, parent_inode)?;
    match dirent.get_block_id(&parent_inode.addr){
        Some(block_id) => {
            todo!("移除这个dir（用空的替换）");
            Some(())
        },
        None =>{
            println!("cannot remove a dir not exists");
            None
        },
    }
}
