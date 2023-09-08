use bitflags::bitflags;
use std::fmt::Debug;

use log::{error, trace};
use serde::{Deserialize, Serialize};
use std::{cmp::min, mem::size_of, time::SystemTime};

use crate::{
    bitmap::{self, alloc_bit},
    block::{
        get_all_valid_blocks, get_block_buffer, write_block, ADDR_TOTAL_SIZE, DIRECT_BLOCK_NUM,
        FIRST_INDIRECT_NUM, FISRT_MAX, INDIRECT_ADDR_NUM, SECOND_MAX,
    },
    dirent::DirEntry,
    simple_fs::{BLOCK_SIZE, DATA_BLOCK, INODE_BLOCK},
};

pub const INODE_SIZE: usize = size_of::<Inode>();
pub const DIRENTRY_SIZE: usize = size_of::<DirEntry>();

pub const MAX_FILE_SIZE: usize = BLOCK_SIZE * (DIRECT_BLOCK_NUM + FISRT_MAX + SECOND_MAX); //可表示文件的最大大小（字节）

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Inode {
    // 内存要对齐！
    inode_type: InodeType,
    mode: FileMode, // 权限
    nlink: u8,
    gid: u16,
    pub inode_id: u16, // inode 号
    uid: u16,
    size: u32,
    time_info: u64,
    // 8个直接，1个一级，一个2级，最大64.25MB, 存的是block id，间接块使用数据区存放【地址】
    pub addr: [u32; ADDR_TOTAL_SIZE],
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]

pub enum InodeType {
    File,
    Diretory,
}

impl Default for InodeType {
    fn default() -> Self {
        Self::Diretory
    }
}

bitflags! {
    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq,Clone,Default)]
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
    pub fn new_root() -> Self {
        assert_eq!(64, INODE_SIZE);
        let inode_id = alloc_bit(bitmap::BitmapType::Inode).unwrap() as u16;
        assert_eq!(0, inode_id, "re-alloc a root inode!");
        let mut root = Self {
            inode_type: InodeType::Diretory,
            mode: FileMode::RDWR,
            inode_id,
            nlink: 0,
            uid: 0,
            gid: 0,
            size: 0,
            addr: [0; ADDR_TOTAL_SIZE],
            time_info: now_secs(),
        };
        // 申请1个data block
        root.alloc_data_blocks();
        assert_eq!(DATA_BLOCK, root.addr[0] as usize);

        let current_dirent = DirEntry::create_dot(&mut root);
        write_block(&current_dirent, root.addr[0] as usize, 0);
        root.cache();
        root
    }

    pub fn alloc(
        inode_type: InodeType,
        parent_inode: &mut Inode,
        mode: FileMode,
        size: u32,
    ) -> Option<Self> {
        // 申请一个inode id
        let inode_id = alloc_bit(bitmap::BitmapType::Inode)? as u16;
        let mut inode = Self {
            inode_type,
            mode,
            inode_id,
            nlink: 0,
            uid: 0,
            gid: 0,
            size,
            addr: [0; ADDR_TOTAL_SIZE],
            time_info: now_secs(),
        };
        // 申请对应大小的data block
        inode.alloc_data_blocks()?;

        if let InodeType::Diretory = inode_type {
            // 申请两个目录项并存放到块中
            let dirs = DirEntry::create_diretory(&mut inode, parent_inode);
            write_block(&dirs, inode.addr[0] as usize, 0);
        }
        // 写入缓存块
        inode.cache();
        Some(inode)
    }

    pub fn alloc_dir(parent_inode: &mut Inode) -> Option<Self> {
        Self::alloc(InodeType::Diretory, parent_inode, FileMode::RDWR, 0)
    }

    /// 一次性为inode申请inode.size大小的block
    fn alloc_data_blocks(&mut self) -> Option<()> {
        let block_nums = self.size as usize / BLOCK_SIZE + 1;
        if block_nums > bitmap::count_valid_data_blocks() {
            // 没有足够的剩余空间
            error!("data not enough");
            return None;
        }
        if block_nums > DIRECT_BLOCK_NUM + FISRT_MAX + SECOND_MAX {
            // 超过了能表示的最大大小
            error!("file size is too large");
            return None;
        }

        // 计算直接块的数量
        let direct_nums = min(DIRECT_BLOCK_NUM, block_nums);
        // 计算一级间接块需要申请的块的数量
        let first_nums = if block_nums > direct_nums {
            min(block_nums - direct_nums, FISRT_MAX)
        } else {
            0
        };
        // 计算二级间接块需要申请的块的数量
        let second_nums = if block_nums > direct_nums + first_nums {
            block_nums - direct_nums - first_nums
        } else {
            0
        };

        let ty = bitmap::BitmapType::Data;
        let start = DATA_BLOCK as u32;
        // 为直接块申请
        for i in 0..direct_nums {
            let block_id = alloc_bit(ty)? + start;
            self.addr[i] = block_id;
        }

        // 为一级间接块申请
        if first_nums > 0 {
            let first_id = alloc_bit(ty)? + start;
            self.addr[DIRECT_BLOCK_NUM] = first_id;

            // 在一级间接块中申请需要的数据块地址
            let mut direct_addrs = Vec::new();
            for _ in 0..first_nums {
                let id = alloc_bit(ty)? + start;
                direct_addrs.push(id);
            }

            // 将申请得到的直接块地址写入间接块中
            write_block(&direct_addrs, first_id as usize, 0);
        }

        // 为二级间接块申请
        if second_nums > 0 {
            let second_id = alloc_bit(ty)? + start;
            self.addr[DIRECT_BLOCK_NUM + FIRST_INDIRECT_NUM] = second_id;

            // 计算需要申请的一级块的数量
            let first_nums = second_nums / INDIRECT_ADDR_NUM + 1;
            let mut first_addrs = Vec::new();
            let mut rest_nums = second_nums;

            for _ in 0..first_nums {
                // 申请一级间接地址并暂存
                let first_id = alloc_bit(ty)? + start;
                first_addrs.push(first_id);

                // 在一级间接块中申请需要的数据块地址
                let mut direct_addrs = Vec::new();
                for _ in 0..min(rest_nums, FISRT_MAX) {
                    let id = alloc_bit(ty)? + start;
                    direct_addrs.push(id);
                }
                rest_nums -= FISRT_MAX;

                // 将申请得到的直接块地址写入一级间接块中
                write_block(&direct_addrs, first_id as usize, 0);
            }
            // 将二级间接块申请得到的地址写入二级块中
            write_block(&first_addrs, second_id as usize, 0);
        }

        Some(())
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

    pub fn cache(&self) {
        let inode_id = self.inode_id as usize;
        let block_id = inode_id / BLOCK_SIZE + INODE_BLOCK;
        let inode_pos = inode_id % 16;
        let start_byte = inode_pos * INODE_SIZE;

        trace!("write inode {} to block {} cache\n", inode_id, block_id);
        write_block(self, block_id, start_byte);
    }

    pub fn linkat(&mut self) {
        self.nlink += 1;
        self.cache();
    }

    pub fn unlinkat(&mut self) {
        self.nlink -= 1;
        self.cache();
    }

    fn is_dir(&self) -> bool {
        matches!(self.inode_type, InodeType::Diretory)
    }

    /// 展示目录信息
    pub fn ls(&self) {
        assert!(self.is_dir());
        for (block_id, _) in &get_all_valid_blocks(&self.addr).unwrap() {
            if *block_id == 0 {
                break;
            }
            println!("\n---------");
            let mut dirs = Vec::new();
            for i in 0..BLOCK_SIZE / DIRENTRY_SIZE {
                let start = i * DIRENTRY_SIZE;
                let end = start + DIRENTRY_SIZE;
                let buffer = get_block_buffer(*block_id as usize, start, end).unwrap();
                // 名字第一个字节为空 说明不是dirent
                if buffer[0] == 0 {
                    break;
                }
                let dir: DirEntry = bincode::deserialize(&buffer).unwrap();
                dirs.push(dir);
            }
            for dir in &dirs {
                println!("{}", dir.get_filename());
            }
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
