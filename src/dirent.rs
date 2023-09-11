use std::{collections::HashSet, hash::Hash};

use log::error;
use serde::{Deserialize, Serialize};

use crate::{
    block::{self, get_all_valid_blocks, get_block_buffer, insert_object, remove_object},
    inode::{Inode, InodeType, DIRENTRY_SIZE},
    simple_fs::{BLOCK_SIZE, SFS},
};

// 文件名和扩展名长度限制（字节）
const NAME_LENGTH_LIMIT: usize = 10;
const EXTENSION_LENGTH_LIMIT: usize = 3;

#[allow(unused)]
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct DirEntry {
    filename: [u8; NAME_LENGTH_LIMIT],       //文件名：10B
    extension: [u8; EXTENSION_LENGTH_LIMIT], //扩展名: 3B
    pub is_dir: bool,
    pub inode_id: u16, //inode号: 2B
}

impl PartialEq for DirEntry {
    fn eq(&self, other: &Self) -> bool {
        self.filename == other.filename && self.extension == other.extension
    }
}

impl Eq for DirEntry {}

impl Hash for DirEntry {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.filename.hash(state);
        self.extension.hash(state);
    }
}

#[allow(unused)]
impl DirEntry {
    /// 在给定inode下生成一个子目录，
    fn new(filename: &str, extension: &str, is_dir: bool, inode_id: u16) -> Option<Self> {
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
                inode_id,
                is_dir,
            })
        }
    }

    pub fn new_temp(filename: &str, extension: &str, is_dir: bool) -> Option<Self> {
        Self::new(filename, extension, is_dir, 0)
    }

    pub fn create_dot(inode: &mut Inode) -> Self {
        let dirent = Self::new(".", "", true, inode.inode_id).unwrap();
        inode.linkat();
        dirent
    }

    pub fn create_dot_dot(inode: &mut Inode) -> Self {
        let dirent = Self::new("..", "", true, inode.inode_id).unwrap();
        inode.linkat();
        dirent
    }

    pub fn create_diretory(current_inode: &mut Inode, parent_inode: &mut Inode) -> (Self, Self) {
        (
            Self::create_dot(current_inode),
            Self::create_dot_dot(parent_inode),
        )
    }

    /// 返回一个dirent数组，以及所在的block及其块等级, 以及是否是目录
    pub fn get_all_dirent(
        inode: &Inode,
    ) -> Option<Vec<(block::BlockLevel, block::BlockIDType, Self)>> {
        let mut dirs = Vec::new();
        for (level, block_id, _) in &get_all_valid_blocks(inode)? {
            if *block_id == 0 {
                break;
            }
            for i in 0..BLOCK_SIZE / DIRENTRY_SIZE {
                let start = i * DIRENTRY_SIZE;
                let end = start + DIRENTRY_SIZE;
                let buffer = get_block_buffer(*block_id as usize, start, end)?;
                // 名字第一个字节为空 说明不是dirent
                if buffer[0] == 0 {
                    continue;
                }
                let dir: DirEntry = bincode::deserialize(&buffer).ok()?;
                dirs.push((*level, *block_id, dir));
            }
        }
        Some(dirs)
    }

    /// 查找给定inode下的同名dirent。如果dirent存在，更新其inode id
    /// 返回所在dirent本身所在的block id（而非目录项所指的inode拥有的空间）和level，
    /// 否则none
    ///
    /// 相当于查找该inode下是否存在给定的dirent
    pub fn get_block_id(
        &mut self,
        inode: &Inode,
    ) -> Option<(block::BlockLevel, block::BlockIDType)> {
        Self::get_all_dirent(inode)
            .unwrap()
            .iter()
            .find_map(|(level, block_id, dir)| {
                if self == dir {
                    // 找到之后更新一下对应的inode id
                    self.inode_id = dir.inode_id;
                    Some((*level, *block_id))
                } else {
                    None
                }
            })
    }

    // 返回dirent的名称 以XXX.abc的形式
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

    /// 递归清空该目录下的所有inode和dirent
    pub fn clear_dir(&mut self) {
        //0. 收集目录下的inode并分类
        let inode = Inode::read(self.inode_id as usize).unwrap();
        let mut dirents = Self::get_all_dirent(&inode).unwrap();
        let mut dir_inodes = Vec::new();
        let mut file_inodes = Vec::new();
        let mut trash_dirs = HashSet::new();
        for (_, _, dirent) in &dirents {
            let mut inode_inside = Inode::read(dirent.inode_id as usize).unwrap();
            match inode_inside.inode_type {
                InodeType::File => {
                    file_inodes.push(inode_inside);
                    // 将目录下类型是文件的目录项删掉，只保留类型为目录的dirent
                    trash_dirs.insert(dirent.clone());
                }
                InodeType::Diretory => {
                    // 单独为上级目录unlinkat
                    if dirent.is_parent() {
                        inode_inside.unlinkat();
                    }
                    // 不要把特殊目录放进去,以免重复删除
                    if !dirent.is_special() {
                        dir_inodes.push(inode_inside);
                    }
                    // 如果该目录项是特殊目录，也从dirents中移除 以免反复递归删除
                    else {
                        trash_dirs.insert(dirent.clone());
                    }
                }
            }
        }
        // 删除刚才加入的需要删除的dirent（文件类型和特殊目录）
        dirents.retain(|(_, _, dirent)| !trash_dirs.contains(dirent));

        //1.1 清除文件inode及其所占有的所有区块
        for fnode in &mut file_inodes {
            fnode.dealloc();
        }

        //1.2.1 递归清空非特殊目录（此时dirents不包含特殊目录）
        for (_, _, dir) in &mut dirents {
            dir.clear_dir();
        }

        //1.2.2 清除目录inode，同时unlinkat,(因为包含了特殊目录指向的inode，所以父级inode的nlink会-1)
        for dnode in &mut dir_inodes {
            // 注意不要把父级inode给dealloc了
            dnode.dealloc();
        }
    }

    pub fn is_current(&self) -> bool {
        self.get_filename() == "."
    }

    pub fn is_parent(&self) -> bool {
        self.get_filename() == ".."
    }
    /// 判断是否是特殊目录
    pub fn is_special(&self) -> bool {
        let name = self.get_filename();
        name == "." || name == ".."
    }
}

pub fn make_directory(name: &str, parent_inode: &mut Inode) -> Option<()> {
    if is_special_dir(name) {
        println!("cannot make such diretory");
        return None;
    }
    // 生成一个名为name的dirent存在父节点的block中
    let (filename, ext) = split_name(name);
    let mut dirent = DirEntry::new_temp(filename, ext, true)?;
    // 判断是否存在同名目录项
    if dirent.get_block_id(parent_inode).is_some() {
        println!("diretory {} already exist", name);
        return None;
    }
    // 为新生成的目录项 申请inode
    let new_node = Inode::alloc_dir(parent_inode)?;
    // 录入新的到的inode id
    dirent.inode_id = new_node.inode_id;
    // 为当前父节点持有的block添加一个目录项
    insert_object(&dirent, parent_inode)?;
    Some(())
}

pub fn remove_directory(name: &str, parent_inode: &mut Inode) -> Option<()> {
    if is_special_dir(name) {
        println!("cannot make such diretory");
        return None;
    }
    let (filename, ext) = split_name(name);
    // 创建一个临时dirent来查找同名目录项
    let mut dirent = DirEntry::new_temp(filename, ext, true)?;
    match dirent.get_block_id(parent_inode) {
        Some((level, block_id)) => {
            //找到了同名目录项
            remove_object(&dirent, block_id as usize, level, parent_inode);
            dirent.clear_dir();
            // 最后dealloc一下目录自己的inode
            let mut dir_inode = Inode::read(dirent.inode_id as usize)?;
            dir_inode.dealloc();
            Some(())
        }
        None => {
            println!("cannot remove a dir: {} which not exists", name);
            None
        }
    }
}

/// 进入某目录（将current inode更换为所指目录项的inode)
pub fn cd(path: &str) -> Option<()> {
    let paths: Vec<&str> = path.split('/').collect();
    let mut current_inode = SFS.lock().current_inode.clone();
    // 循环复合目录
    for &path in &paths {
        // 找不到了便返回None
        current_inode = try_cd(path, &current_inode)?;
    }
    let mut fs = SFS.lock();
    fs.current_inode = current_inode;
    // 调整当前目录
    for &path in &paths {
        match path {
            "." => {}
            ".." => {
                let idx = fs.cwd.rfind('/').unwrap();
                fs.cwd.replace_range(idx.., "");
            }
            _ => fs.cwd.push_str(&["/", path].concat()),
        }
    }
    Some(())
}

fn try_cd(name: &str, current_inode: &Inode) -> Option<Inode> {
    let (filename, ext) = if is_special_dir(name) {
        (name, "")
    } else {
        split_name(name)
    };
    let mut dirent = DirEntry::new_temp(filename, ext, true)?;
    // 判断是否存在同名目录项
    if dirent.get_block_id(current_inode).is_some() {
        //找到了同名目录项
        let target_inode = Inode::read(dirent.inode_id as usize)?;
        if let InodeType::File = target_inode.inode_type {
            println!("{} is not a directory", name);
            return None;
        }
        Some(target_inode)
    } else {
        println!("no such diretory");
        None
    }
}

fn is_special_dir(name: &str) -> bool {
    name == "." || name == ".."
}

// 分割输入的名字
pub fn split_name(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some(it) => it,
        None => (name, ""),
    }
}
