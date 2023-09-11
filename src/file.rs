use std::io::{self, BufRead};

use log::error;

use crate::{
    block::{get_all_blocks, insert_object, remove_object, write_block},
    dirent::{self, DirEntry},
    inode::{FileMode, Inode, InodeType, MAX_FILE_SIZE},
    simple_fs::BLOCK_SIZE,
};

pub fn create_file(name: &str, mode: FileMode, parent_inode: &mut Inode) -> Option<()> {
    let (filename, extension) = dirent::split_name(name);
    // 查找重名文件
    let mut dirent = DirEntry::new_temp(filename, extension, false)?;
    if dirent.get_block_id(parent_inode).is_some() {
        println!("file already exists");
        return None;
    }

    // 打开io流接受输入（以空行结束）
    let inputs = read_from_cli();
    if inputs.len() > MAX_FILE_SIZE {
        println!("File size limit exceed");
        return None;
    }

    let size = inputs.len() as u32;
    // 按block大小分割
    let input_vecs = split_inputs(inputs);
    // 按大小申请inode
    let mut inode = Inode::alloc(InodeType::File, parent_inode, mode, size)?;
    inode.linkat();
    
    dirent.inode_id = inode.inode_id;
    // 将文件写入block中
    let blocks = get_all_blocks(&inode)?;
    assert!(blocks.len() >= input_vecs.len());
    for (i, (_, block_id, _)) in blocks.into_iter().enumerate() {
        write_block(&input_vecs[i], block_id as usize, 0);
    }
    // 将目录项写入目录中
    // 为当前父节点持有的block添加一个目录项
    insert_object(&dirent, parent_inode)?;
    Some(())
}

pub fn remove_file(name: &str, parent_inode: &mut Inode) -> Option<()> {
    let (filename, extension) = dirent::split_name(name);
    // 查找重名文件
    let mut dirent = DirEntry::new_temp(filename, extension, false)?;
    match dirent.get_block_id(parent_inode) {
        None => {
            println!("no such file");
            None
        }
        Some((level, block_id)) => {
            // 删除目录项
            remove_object(&dirent, block_id as usize, level, parent_inode);
            // 释放inode
            let mut inode = Inode::read(dirent.inode_id as usize)?;
            inode.dealloc();
            Some(())
        }
    }
}

pub fn open_file(name: &str, parent_inode: &Inode) -> Option<String> {
    let (filename, extension) = dirent::split_name(name);
    // 查找重名文件
    let mut dirent = DirEntry::new_temp(filename, extension, false)?;
    if dirent.get_block_id(parent_inode).is_none() {
        println!("no such file");
        None
    } else {
        //获取内容
        let inode = Inode::read(dirent.inode_id as usize)?;
        let blocks = get_all_blocks(&inode)?;
        let mut content = String::new();
        for (_, _, block) in blocks {
            let string: String = bincode::deserialize(&block).ok()?;
            content.push_str(&string);
        }
        Some(content)
    }
}

pub fn copy_file(source_path:&str,target_path:&str) -> Option<()> {
    Some(())
}

fn read_from_cli() -> String {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut inputs = String::new();
    loop {
        if let Some(Ok(input)) = lines.next() {
            // 如果输入是空行，则退出
            if input.trim().is_empty() {
                break;
            }
            inputs.push_str(&input);
        } else {
            error!("cannot read stdin");
            break;
        }
    }
    inputs
}

fn split_inputs(inputs: String) -> Vec<String> {
    inputs
        .chars()
        .collect::<Vec<char>>()
        .chunks(BLOCK_SIZE)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect()
}
