use std::io::{Error, ErrorKind};

use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
};

use crate::{
    block::{
        get_all_blocks, get_all_valid_blocks, insert_object, remove_object,
        write_file_content_to_blocks,
    },
    dirent::{self, DirEntry},
    fs_constants::*,
    inode::{FileMode, Inode, InodeType},
    user::{self, UserIdType},
};

/// 创建文件，存在同名文件时err
pub async fn create_file(
    name: &str,
    mode: FileMode,
    parent_inode: &mut Inode,
    is_copy: bool,
    content: &str,
    socket: &mut TcpStream,
    user_id: (UserIdType, UserIdType),
) -> Result<(), Error> {
    let (filename, extension) = dirent::split_name(name);
    // 查找重名文件
    let mut dirent = DirEntry::new_temp(filename, extension, false)?;
    if dirent
        .get_block_id_and_try_update(parent_inode)
        .await
        .is_ok()
    {
        return Err(Error::new(ErrorKind::AlreadyExists, "file already exists"));
    }

    let inputs;
    // 如果是copy模式，则不需要使用stdio
    if is_copy {
        inputs = content.to_owned();
    } else {
        // 建立临时socket，端口随机
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        // 2.ex1.1 向client告知需要输入内容，同时发送端口
        let addr = listener.local_addr()?.to_string();
        let msg = [utils::INPUT_FILE_CONTENT, &addr].concat();
        socket.write_all(msg.as_bytes()).await?;
        // 2.ex1.2 client 读取文件内容
        info!("receiving contents through {}", addr);
        inputs = utils::receive_content(&listener).await?;
        if inputs.len() > MAX_FILE_SIZE {
            return Err(Error::new(ErrorKind::OutOfMemory, "File size limit exceed"));
        }
    }
    let size = inputs.len() as u32;
    // 按block大小分割
    let input_vecs = split_inputs(inputs);
    // 按大小申请inode
    let mut inode = Inode::alloc(
        InodeType::File,
        parent_inode,
        mode,
        size,
        user_id.0,
        user_id.1,
    )
    .await?;
    inode.linkat().await;

    dirent.inode_id = inode.inode_id;
    // 将文件写入block中
    let blocks = get_all_blocks(&inode).await?;
    assert!(blocks.len() >= input_vecs.len());
    let block_ids: Vec<_> = blocks.iter().map(|(_, id, _)| *id as usize).collect();
    write_file_content_to_blocks(&input_vecs, &block_ids).await?;

    // 将目录项写入目录中
    // 为当前父节点持有的block添加一个目录项
    insert_object(&dirent, parent_inode).await?;
    Ok(())
}

/// 删除文件，不存在时err
pub async fn remove_file(
    name: &str,
    parent_inode: &mut Inode,
    gid: UserIdType,
) -> Result<(), Error> {
    let (filename, extension) = dirent::split_name(name);
    // 查找重名文件
    let mut dirent = DirEntry::new_temp(filename, extension, false)?;
    match dirent.get_block_id_and_try_update(parent_inode).await {
        Err(err) => Err(err),
        Ok((level, block_id)) => {
            if dirent.is_dir {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    format!("{} is not a file", name),
                ));
            }
            let mut inode = Inode::read(dirent.inode_id as usize).await?;
            if !user::able_to_modify(gid, inode.gid) {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    "Insufficient user permissions",
                ));
            }
            // 释放inode
            inode.dealloc().await;
            // 删除目录项
            remove_object(&dirent, block_id as usize, level, parent_inode).await?;
            Ok(())
        }
    }
}

/// 获取文件内容
pub async fn get_file_content(name: &str, parent_inode: &Inode) -> Result<String, Error> {
    let (filename, extension) = dirent::split_name(name);
    // 查找重名文件
    let mut dirent = DirEntry::new_temp(filename, extension, false)?;
    if dirent
        .get_block_id_and_try_update(parent_inode)
        .await
        .is_err()
    {
        Err(Error::new(ErrorKind::NotFound, "no such file"))
    } else if dirent.is_dir {
        Err(Error::new(
            ErrorKind::PermissionDenied,
            "cannot open a directory",
        ))
    } else {
        //获取内容
        let inode = Inode::read(dirent.inode_id as usize).await?;
        let blocks = get_all_valid_blocks(&inode).await?;
        let bytes: Vec<_> = blocks.into_iter().flat_map(|(_, _, block)| block).collect();
        let content = String::from_utf8_lossy(&bytes)
            .trim_end_matches('\0')
            .to_string();
        Ok(content)
    }
}

/// 将input string按块大小分割成数组
fn split_inputs(inputs: String) -> Vec<String> {
    let ch = inputs.as_bytes().chunks(BLOCK_SIZE);
    let mut result = Vec::new();
    for chunk in ch {
        let chunk_str = std::str::from_utf8(chunk).expect("Invalid UTF-8 sequence");
        result.push(chunk_str.to_string());
    }
    result
}
