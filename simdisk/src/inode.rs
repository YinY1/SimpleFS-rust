use bitflags::bitflags;

use serde::{Deserialize, Serialize};
use std::{
    cmp::min,
    io::{Error, ErrorKind},
    sync::Arc,
    time::SystemTime,
};

use crate::{
    bitmap::{self, alloc_bit, dealloc_data_bit, dealloc_inode_bit, BitmapType},
    block::{deserialize, get_block_buffer, write_block, BlockIDType},
    dirent::DirEntry,
    fs_constants::*,
    simple_fs::SFS,
    user,
};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Inode {
    // 内存要对齐！
    pub inode_id: u16, // inode 号
    pub inode_type: InodeType,
    mode: FileMode, // 权限
    nlink: u8,      // 硬连接数
    pub gid: u16,   // 组id
    uid: u16,       // 用户id
    size: u32,      // 文件大小
    time_info: u64, // 时间戳
    // 8个直接，1个一级，一个2级，最大64.25MB, 存的是block id，间接块使用数据区存放【地址】
    pub addr: [BlockIDType; ADDR_TOTAL_SIZE],
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
    // 创建根节点
    pub async fn new_root() -> Self {
        assert_eq!(64, INODE_SIZE);
        let inode_id = alloc_bit(BitmapType::Inode).await.unwrap() as u16;
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
        root.alloc_data_blocks().await.unwrap();
        assert_eq!(DATA_START_BLOCK, root.addr[0] as usize);

        let current_dirent = DirEntry::create_dot(&mut root).await;
        let _ = write_block(&current_dirent, root.addr[0] as usize, 0).await;
        root.cache().await;
        root
    }

    // 申请一个inode
    pub async fn alloc(
        inode_type: InodeType,
        parent_inode: &mut Inode,
        mode: FileMode,
        size: u32,
        gid: u16,
        uid: u16,
    ) -> Result<Self, Error> {
        // 申请一个inode id
        let inode_id = alloc_bit(BitmapType::Inode).await? as u16;
        let mut inode = Self {
            inode_type,
            mode,
            inode_id,
            nlink: 0,
            uid,
            gid,
            size,
            addr: [0; ADDR_TOTAL_SIZE],
            time_info: now_secs(),
        };
        // 申请对应大小的data block
        inode.alloc_data_blocks().await?;

        if let InodeType::Diretory = inode_type {
            // 申请两个目录项并存放到块中
            let dirs = DirEntry::create_diretory(&mut inode, parent_inode).await;
            let _ = write_block(&dirs, inode.addr[0] as usize, 0).await;
        }
        // 写入缓存块
        inode.cache().await;
        Ok(inode)
    }

    pub async fn alloc_dir(parent_inode: &mut Inode, gid: u16, uid: u16) -> Result<Self, Error> {
        Self::alloc(
            InodeType::Diretory,
            parent_inode,
            FileMode::RDWR,
            0,
            gid,
            uid,
        )
        .await
    }

    /// 移除自身inode，从位图中dealloc，清空所拥有的数据（递归dealloc所拥有的block及其内容）
    pub async fn dealloc(&mut self) {
        //0.1 dealloc 自己
        assert!(dealloc_inode_bit(self.inode_id as usize).await);
        //0.2 unlink(主要针对目录.和..)
        self.unlinkat().await;

        //1. dealloc直接块
        for i in 0..DIRECT_BLOCK_NUM {
            let id = self.addr[i] as usize;
            if id == 0 {
                return;
            }
            dealloc_data_bit(id).await;
        }

        //2.1 dealloc一级块
        let first_id = self.get_first_id();
        if first_id == 0 {
            return;
        }
        dealloc_data_bit(first_id).await;
        //2.2 然后dealloc一级块中的每个直接块
        dealloc_first_blocks(first_id).await;

        //3.1 dealloc二级块
        let second_id = self.get_second_id();
        if second_id == 0 {
            return;
        }
        dealloc_data_bit(second_id).await;
        //3.2 再dealloc二级块的一级块
        let mut first_ids = Vec::new();
        for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
            let start = i * BLOCK_ADDR_SIZE;
            let end = start + BLOCK_ADDR_SIZE;
            let first_block = get_block_buffer(second_id, start, end).await.unwrap();
            let first_id: u32 = bincode::deserialize(&first_block).unwrap();
            first_ids.push(first_id);
            dealloc_data_bit(first_id as usize).await;
        }
        //3.3 最后dealloc二级块中的每个一级块的直接块
        for first_id in &first_ids {
            dealloc_first_blocks(*first_id as usize).await;
        }
    }

    pub fn get_first_id(&self) -> usize {
        self.addr[DIRECT_BLOCK_NUM] as usize
    }

    pub fn set_first_id(&mut self, first_id: BlockIDType) {
        self.addr[DIRECT_BLOCK_NUM] = first_id;
    }

    pub fn get_second_id(&self) -> usize {
        self.addr[DIRECT_BLOCK_NUM + FIRST_INDIRECT_NUM] as usize
    }

    pub fn set_second_id(&mut self, second_id: BlockIDType) {
        self.addr[DIRECT_BLOCK_NUM + FIRST_INDIRECT_NUM] = second_id;
    }

    /// 一次性为inode申请inode.size大小的block
    async fn alloc_data_blocks(&mut self) -> Result<(), Error> {
        let block_nums = if self.size == 0 {
            1
        } else {
            (self.size as f64 / BLOCK_SIZE as f64).ceil() as usize
        };
        if block_nums > bitmap::count_valid_data_blocks().await {
            // 没有足够的剩余空间
            error!("data not enough");
            return Err(Error::new(ErrorKind::OutOfMemory, "no enough block"));
        }
        if block_nums > DIRECT_BLOCK_NUM + FISRT_MAX + SECOND_MAX {
            // 超过了能表示的最大大小
            error!("file size is too large");
            return Err(Error::new(ErrorKind::OutOfMemory, "file size is too large"));
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

        let ty = BitmapType::Data;
        let start = DATA_START_BLOCK as u32;
        // 为直接块申请
        for i in 0..direct_nums {
            let block_id = alloc_bit(ty).await? + start;
            self.addr[i] = block_id;
        }

        // 为一级间接块申请
        if first_nums > 0 {
            let first_id = alloc_bit(ty).await? + start;
            self.set_first_id(first_id);

            // 在一级间接块中申请需要的数据块地址
            let mut direct_addrs = Vec::new();
            for _ in 0..first_nums {
                let id = alloc_bit(ty).await? + start;
                direct_addrs.push(id);
            }

            // 将申请得到的直接块地址写入间接块中
            write_block(&direct_addrs, first_id as usize, 0).await?;
        }

        // 为二级间接块申请
        if second_nums > 0 {
            let second_id = alloc_bit(ty).await? + start;
            self.addr[DIRECT_BLOCK_NUM + FIRST_INDIRECT_NUM] = second_id;

            // 计算需要申请的一级块的数量
            let first_nums = second_nums / INDIRECT_ADDR_NUM + 1;
            let mut first_addrs = Vec::new();
            let mut rest_nums = second_nums;

            for _ in 0..first_nums {
                // 申请一级间接地址并暂存
                let first_id = alloc_bit(ty).await? + start;
                first_addrs.push(first_id);

                // 在一级间接块中申请需要的数据块地址
                let mut direct_addrs = Vec::new();
                for _ in 0..min(rest_nums, FISRT_MAX) {
                    let id = alloc_bit(ty).await? + start;
                    direct_addrs.push(id);
                }
                rest_nums -= FISRT_MAX;

                // 将申请得到的直接块地址写入一级间接块中
                write_block(&direct_addrs, first_id as usize, 0).await?;
            }
            // 将二级间接块申请得到的地址写入二级块中
            write_block(&first_addrs, second_id as usize, 0).await?;
        }

        Ok(())
    }

    /// 直接从block读取inode信息
    pub async fn read(inode_id: usize) -> Result<Self, Error> {
        let block_id = inode_id / BLOCK_SIZE + INODE_START_BLOCK;
        let inode_pos = inode_id % 16;
        let start_byte = inode_pos * INODE_SIZE;
        let end_byte = start_byte + INODE_SIZE;

        // 一个Inode 64B
        let buffer = get_block_buffer(block_id, start_byte, end_byte).await?;
        deserialize(&buffer)
    }

    ///将inode写入缓存中
    async fn cache(&self) {
        let inode_id = self.inode_id as usize;
        let block_id = inode_id / BLOCK_SIZE + INODE_START_BLOCK;
        let inode_pos = inode_id % 16;
        let start_byte = inode_pos * INODE_SIZE;

        trace!("write inode {} to block {} cache\n", inode_id, block_id);
        let _ = write_block(self, block_id, start_byte).await;
    }

    pub async fn linkat(&mut self) {
        self.nlink += 1;
        self.cache().await;
    }

    pub async fn unlinkat(&mut self) {
        self.nlink -= 1;
        self.cache().await;
    }

    fn is_dir(&self) -> bool {
        matches!(self.inode_type, InodeType::Diretory)
    }

    /// 展示当前inode目录的信息
    pub async fn ls(&self, detail: bool) -> String {
        assert!(self.is_dir());
        let mut dir_infos = String::new();
        for (_, _, dir) in DirEntry::get_all_dirent(self).await.unwrap().iter() {
            let mut name = dir.get_filename();
            if dir.is_dir {
                name.push('/');
            }
            if detail {
                let inode = Self::read(dir.inode_id as usize).await.unwrap();
                let addr = inode.addr[0] as usize * BLOCK_SIZE;
                let time = cal_date(inode.time_info);
                let fs = Arc::clone(&SFS);
                let fs_read_lock = fs.read().await;
                let username = fs_read_lock.get_username(inode.uid).unwrap();
                let mode = if user::able_to_modify(fs_read_lock.current_user.gid, inode.gid) {
                    inode.mode
                } else {
                    FileMode::RDONLY
                };

                let infos = format!(
                    "\taddr:{:#x}\tcreated: {:#?}\t{:?}\t\tBy: {:?}",
                    addr, time, mode, username,
                );
                name.push_str(&infos);
            }
            dir_infos.push_str(&name);
            dir_infos.push('\n');
        }
        trace!("ls ok");
        dir_infos
    }
}

async fn dealloc_first_blocks(first_id: usize) {
    for i in 0..BLOCK_SIZE / BLOCK_ADDR_SIZE {
        let start = i * BLOCK_ADDR_SIZE;
        let end = start + BLOCK_ADDR_SIZE;
        let direct_block = get_block_buffer(first_id, start, end).await.unwrap();
        let id: u32 = bincode::deserialize(&direct_block).unwrap();
        dealloc_data_bit(id as usize).await;
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn cal_date(timestamp: u64) -> chrono::NaiveDate {
    chrono::NaiveDateTime::from_timestamp_opt(timestamp as i64, 0)
        .unwrap()
        .date()
}
