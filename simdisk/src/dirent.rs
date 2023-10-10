use std::{
    collections::HashSet,
    hash::Hash,
    io::{Error, ErrorKind},
};

use async_recursion::async_recursion;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use crate::{
    block::{
        self, deserialize, get_all_valid_blocks, get_blocks_buffers, insert_object, remove_object,
    },
    fs_constants::*,
    inode::{Inode, InodeIdType, InodeType},
    user::{self, UserIdType},
};

#[allow(unused)]
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct DirEntry {
    filename: [u8; NAME_LENGTH_LIMIT],       //文件名：10B
    extension: [u8; EXTENSION_LENGTH_LIMIT], //扩展名: 3B
    pub is_dir: bool,                        //目录标志：1B
    pub inode_id: InodeIdType,               //inode号: 2B
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
    fn new(
        filename: &str,
        extension: &str,
        is_dir: bool,
        inode_id: InodeIdType,
    ) -> Result<Self, Error> {
        if filename.len() > NAME_LENGTH_LIMIT {
            error!("filename TOO LONG");
            Err(Error::new(ErrorKind::InvalidInput, "filename TOO LONG"))
        } else if extension.len() > EXTENSION_LENGTH_LIMIT {
            error!("extension TOO LONG");
            Err(Error::new(ErrorKind::InvalidInput, "extension TOO LONG"))
        } else {
            let mut filename_ = [0; NAME_LENGTH_LIMIT];
            filename_[..filename.len()].copy_from_slice(filename.as_bytes());
            let mut extension_ = [0; EXTENSION_LENGTH_LIMIT];
            extension_[..extension.len()].copy_from_slice(extension.as_bytes());

            Ok(Self {
                filename: filename_,
                extension: extension_,
                inode_id,
                is_dir,
            })
        }
    }

    /// 临时的dirent用于比较文件名是否相等
    pub fn new_temp(filename: &str, extension: &str, is_dir: bool) -> Result<Self, Error> {
        Self::new(filename, extension, is_dir, 0)
    }

    /// 创建特殊目录.
    pub async fn create_dot(inode: &mut Inode) -> Self {
        let dirent = Self::new(".", "", true, inode.inode_id).unwrap();
        inode.linkat().await;
        dirent
    }

    /// 创建特殊目录..
    pub async fn create_dot_dot(inode: &mut Inode) -> Self {
        let dirent = Self::new("..", "", true, inode.inode_id).unwrap();
        inode.linkat().await;
        dirent
    }

    /// 创建两个特殊目录
    pub async fn create_special_diretories(
        current_inode: &mut Inode,
        parent_inode: &mut Inode,
    ) -> (Self, Self) {
        (
            Self::create_dot(current_inode).await,
            Self::create_dot_dot(parent_inode).await,
        )
    }

    /// 返回一个数组，包含块等级，所在的block id，以及目录项
    pub async fn get_all_dirent(
        inode: &Inode,
    ) -> Result<Vec<(block::BlockLevel, block::BlockIDType, Self)>, Error> {
        let mut dirs = Vec::new();
        for (level, block_id, _) in &get_all_valid_blocks(inode).await? {
            if *block_id == 0 {
                break;
            }
            // 取出对应block的所有dirent buffer
            let mut block_args = Vec::new();
            for i in 0..BLOCK_SIZE / DIRENTRY_SIZE {
                let start = i * DIRENTRY_SIZE;
                let end = start + DIRENTRY_SIZE;
                block_args.push((*block_id as usize, start, end));
            }
            let buffers = get_blocks_buffers(&block_args).await?;

            // 将buffer反序列化成dirent
            for buffer in buffers {
                // 名字第一个字节为空 说明不是dirent
                if buffer[0] == 0 {
                    continue;
                }
                let dir: DirEntry = deserialize(&buffer)?;
                dirs.push((*level, *block_id, dir));
            }
        }
        Ok(dirs)
    }

    /// 查找给定inode下的同名dirent。如果dirent存在，更新其inode id
    /// 返回所在dirent本身所在的block id（而非目录项所指的inode拥有的空间）和level，
    /// 否则err
    ///
    /// 相当于查找该inode下是否存在给定的dirent
    pub async fn get_block_id_and_try_update(
        &mut self,
        inode: &Inode,
    ) -> Result<(block::BlockLevel, block::BlockIDType), Error> {
        Self::get_all_dirent(inode)
            .await
            .unwrap()
            .iter()
            .find_map(|(level, block_id, dir)| {
                if self == dir {
                    // 找到之后更新一下对应的inode id和类型
                    self.inode_id = dir.inode_id;
                    self.is_dir = dir.is_dir;
                    Some((*level, *block_id))
                } else {
                    None
                }
            })
            .ok_or(Error::new(ErrorKind::NotFound, "dirent not found"))
    }

    /// 返回dirent的名称 以XXX.abc的形式
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
    #[async_recursion]
    pub async fn clear_dir(&mut self) {
        //0. 收集目录下的inode并分类
        let inode = Inode::read(self.inode_id as usize).await.unwrap();
        let mut dirents = Self::get_all_dirent(&inode).await.unwrap();
        let mut dir_inodes = Vec::new();
        let mut file_inodes = Vec::new();
        let mut trash_dirs = HashSet::new();
        for (_, _, dirent) in &dirents {
            let mut inode_inside = Inode::read(dirent.inode_id as usize).await.unwrap();
            match inode_inside.inode_type {
                InodeType::File => {
                    file_inodes.push(inode_inside);
                    // 将目录下类型是文件的目录项删掉，只保留类型为目录的dirent
                    trash_dirs.insert(dirent.clone());
                }
                InodeType::Diretory => {
                    // 单独为上级目录unlinkat
                    if dirent.is_parent() {
                        inode_inside.unlinkat().await;
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
            fnode.dealloc().await;
        }
        trace!("dealloc file nodes ok");

        //1.2.1 递归清空非特殊目录（此时dirents不包含特殊目录）
        for (_, _, dir) in &mut dirents {
            let fname = dir.filename;
            let name = String::from_utf8_lossy(&fname);
            trace!("try clear {}", name);
            dir.clear_dir().await;
            trace!("clear {} ok", name);
        }

        //1.2.2 清除目录inode，同时unlinkat,(因为包含了特殊目录指向的inode，所以父级inode的nlink会-1)
        for dnode in &mut dir_inodes {
            // 注意不要把父级inode给dealloc了
            dnode.dealloc().await;
            trace!("dealloc {} ok", dnode.inode_id);
        }
        trace!("clear ok");
    }

    pub fn is_current(&self) -> bool {
        self.get_filename() == "."
    }

    pub fn is_parent(&self) -> bool {
        self.get_filename() == ".."
    }
    /// 判断是否是特殊目录
    pub fn is_special(&self) -> bool {
        is_special_dir(&self.get_filename())
    }
}

/// 创建目录，失败时返回错误信息
pub async fn make_directory(
    name: &str,
    parent_inode: &mut Inode,
    gid: UserIdType,
    uid: UserIdType,
) -> Result<(), Error> {
    if is_special_dir(name) {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            "cannot make such diretory",
        ));
    }
    // 生成一个名为name的dirent存在父节点的block中
    let (filename, ext) = split_name(name);
    let mut dirent = DirEntry::new_temp(filename, ext, true)?;
    // 判断是否存在同名目录项
    if dirent
        .get_block_id_and_try_update(parent_inode)
        .await
        .is_ok()
    {
        let err = format!("diretory {} already exist", name);
        return Err(Error::new(ErrorKind::AlreadyExists, err));
    }
    // 为新生成的目录项 申请inode
    let mut new_node = Inode::alloc_dir_inode(parent_inode, gid, uid).await?;
    new_node.linkat().await;
    // 录入新的到的inode id
    dirent.inode_id = new_node.inode_id;
    // 为当前父节点持有的block添加一个目录项
    insert_object(&dirent, parent_inode).await?;
    trace!("make dir ok");
    Ok(())
}

/// 删除目录
pub async fn remove_directory(
    name: &str,
    parent_inode: &mut Inode,
    socket: &mut TcpStream,
    gid: UserIdType,
) -> Result<(), Error> {
    if is_special_dir(name) {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            "cannot make such diretory",
        ));
    }
    let (filename, ext) = split_name(name);
    // 创建一个临时dirent来查找同名目录项
    let mut dirent = DirEntry::new_temp(filename, ext, true)?;
    match dirent.get_block_id_and_try_update(parent_inode).await {
        // 判断目录是否非空
        Ok((level, block_id)) => {
            //找到了同名目录项
            let mut dir_inode = Inode::read(dirent.inode_id as usize).await?;
            // 不能越权
            if !user::able_to_modify(gid, dir_inode.gid) {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    "Insufficient user permissions",
                ));
            }
            let dirs = DirEntry::get_all_dirent(&dir_inode).await?;
            for (_, _, dirent) in dirs {
                if !dirent.is_special() {
                    // send指令
                    socket.write_all(shell::COMMAND_CONFIRM.as_bytes()).await?;
                    // 2.ex2 从client 等待确认指令
                    let mut response = [0; 8];
                    let n = socket.read(&mut response).await?;
                    if n == 0 {
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "cannot read from client",
                        ));
                    }
                    let response = String::from_utf8_lossy(&response).replace('\0', "");
                    match response.trim() {
                        "y" | "Y" => break,
                        _ => {
                            info!("remove cancel, input {}", response);
                            return Ok(());
                        }
                    }
                }
            }
            trace!("answer is YES, do remove");
            remove_object(&dirent, block_id as usize, level, parent_inode).await?;
            dirent.clear_dir().await;
            // 最后dealloc一下目录自己的inode
            dir_inode.dealloc().await;
            trace!("remove dir ok");
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// 进入某目录（将current inode更换为所指目录项的inode), 如果有错误信息则返回
pub async fn cd(path: &str, current_inode: &Inode) -> Result<Inode, Error> {
    //将绝对路径分割为多段
    let paths: Vec<&str> = path.split('/').collect();
    let mut current_inode = current_inode.clone();
    // 循环复合目录(除去~)
    for &path in &paths[1..] {
        // 找不到了便返回Err
        match try_cd(path, &current_inode).await {
            Ok(inode) => current_inode = inode,
            Err(e) => {
                return Err(e);
            }
        }
    }
    Ok(current_inode)
}

/// 尝试进入某目录
async fn try_cd(name: &str, current_inode: &Inode) -> Result<Inode, Error> {
    let (filename, ext) = if is_special_dir(name) {
        (name, "")
    } else {
        split_name(name)
    };
    let mut dirent = DirEntry::new_temp(filename, ext, true)?;
    // 判断是否存在同名目录项
    if dirent
        .get_block_id_and_try_update(current_inode)
        .await
        .is_ok()
    {
        //找到了同名目录项
        let target_inode = Inode::read(dirent.inode_id as usize).await?;
        if let InodeType::File = target_inode.inode_type {
            let err = format!("{} is not a directory", name);
            return Err(Error::new(ErrorKind::PermissionDenied, err));
        }
        Ok(target_inode)
    } else {
        Err(Error::new(ErrorKind::NotFound, "no such diretory"))
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
