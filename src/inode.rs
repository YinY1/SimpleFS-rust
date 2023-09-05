pub struct Inode {
    inode_tyoe: InodeType,
    mode: u8,      // 权限
    inode_id: u16, // inode 号
    nlink: u8,
    uid: u32,
    gid: u32,
    size: u32,
    addr: [u32; 11], // 9个直接，1个一级，一个2级，最大33MB
}

const NAME_LENGTH_LIMIT: usize = 12;

#[allow(unused)]
pub struct DirEntry {
    filename: [u8; NAME_LENGTH_LIMIT + 1], //文件名：13B
    extension: u8,                         //扩展名: 1B
    inode_ud: u16,                         //inode号: 2B
}

pub enum InodeType {
    File,
    Diretory,
}
