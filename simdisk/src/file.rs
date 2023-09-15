use std::io::{Error, ErrorKind};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use crate::{
    block::{get_all_blocks, insert_object, remove_object, write_file_content_to_block},
    dirent::{self, DirEntry},
    fs_constants::*,
    inode::{FileMode, Inode, InodeType},
};

pub async fn create_file(
    name: &str,
    mode: FileMode,
    parent_inode: &mut Inode,
    is_copy: bool,
    content: &str,
    socket: &mut TcpStream,
) -> Result<(), Error> {
    let (filename, extension) = dirent::split_name(name);
    // 查找重名文件
    let mut dirent = DirEntry::new_temp(filename, extension, false)?;
    if dirent.get_block_id(parent_inode).await.is_ok() {
        return Err(Error::new(ErrorKind::AlreadyExists, "file already exists"));
    }

    let inputs;
    // 如果是copy模式，则不需要使用stdio
    if is_copy {
        inputs = content.to_owned();
    } else {
        // 2.ex1.1 向client告知需要输入内容
        socket
            .write_all(shell::INPUT_FILE_CONTENT.as_bytes())
            .await?;
        // 2.ex1.2 client 读取文件内容
        let mut input_buffer = [0; 1024]; // TODO 循环读缓冲区直到读完
        let n = socket.read(&mut input_buffer).await?;
        if n == 0 {
            return Err(Error::new(
                ErrorKind::ConnectionAborted,
                "cannot read file content from client",
            ));
        }
        inputs = String::from_utf8_lossy(&input_buffer).to_string();
        if inputs.len() > MAX_FILE_SIZE {
            return Err(Error::new(ErrorKind::OutOfMemory, "File size limit exceed"));
        }
    }
    let size = inputs.len() as u32;
    // 按block大小分割
    let input_vecs = split_inputs(inputs);
    // 按大小申请inode
    let mut inode = Inode::alloc(InodeType::File, parent_inode, mode, size).await?;
    inode.linkat().await;

    dirent.inode_id = inode.inode_id;
    // 将文件写入block中
    let blocks = get_all_blocks(&inode).await?;
    assert!(blocks.len() >= input_vecs.len());
    for (i, content) in input_vecs.into_iter().enumerate() {
        write_file_content_to_block(content, blocks[i].1 as usize).await?;
    }
    // 将目录项写入目录中
    // 为当前父节点持有的block添加一个目录项
    insert_object(&dirent, parent_inode).await?;
    Ok(())
}

pub async fn remove_file(name: &str, parent_inode: &mut Inode) -> Result<(), Error> {
    let (filename, extension) = dirent::split_name(name);
    // 查找重名文件
    let mut dirent = DirEntry::new_temp(filename, extension, false)?;
    match dirent.get_block_id(parent_inode).await {
        Err(err) => Err(err),
        Ok((level, block_id)) => {
            // 删除目录项
            remove_object(&dirent, block_id as usize, level, parent_inode).await?;
            // 释放inode
            let mut inode = Inode::read(dirent.inode_id as usize).await?;
            inode.dealloc().await;
            Ok(())
        }
    }
}

pub async fn open_file(name: &str, parent_inode: &Inode) -> Result<String, Error> {
    let (filename, extension) = dirent::split_name(name);
    // 查找重名文件
    let mut dirent = DirEntry::new_temp(filename, extension, false)?;
    if dirent.get_block_id(parent_inode).await.is_err() {
        Err(Error::new(ErrorKind::NotFound, "no such file"))
    } else if dirent.is_dir {
        Err(Error::new(
            ErrorKind::PermissionDenied,
            "cannot open a directory",
        ))
    } else {
        //获取内容
        let inode = Inode::read(dirent.inode_id as usize).await?;
        let blocks = get_all_blocks(&inode).await?;
        let mut content = String::new();
        for (_, _, block) in blocks {
            let string = String::from_utf8_lossy(&block).to_string();
            content.push_str(&string);
        }
        Ok(content)
    }
}

/// 将input string按块大小分割成数组
fn split_inputs(inputs: String) -> Vec<String> {
    inputs
        .chars()
        .collect::<Vec<char>>()
        .chunks(BLOCK_SIZE)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect()
}
