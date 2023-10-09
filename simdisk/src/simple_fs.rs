#[allow(unused)]
use log::{debug, error, info, trace};
use std::{
    fs::File,
    io::{Error, Write},
    sync::Arc,
};
use tokio::sync::RwLock;

use crate::{
    bitmap::{count_data_blocks, count_inodes},
    block::{self, BLOCK_CACHE_MANAGER},
    fs_constants::*,
    inode::{self, Inode},
    super_block::SuperBlock,
    user::{User, UserIdGroup, UserInfo},
};

#[allow(unused)]
#[derive(Default)]
pub struct SimpleFileSystem {
    pub root_inode: Inode,       //文件系统的根节点
    pub super_block: SuperBlock, //超级块的信息
    pub current_inode: Inode,    // 临时记录操作的当前节点
    pub user_infos: User,        // 文件系统的用户信息
}

impl SimpleFileSystem {
    /// 从文件系统中读出相关信息
    pub async fn read(&mut self) {
        trace!("read SFS");
        let root_inode = Inode::read(0).await.unwrap();
        *self = Self {
            current_inode: root_inode.clone(),
            root_inode,
            super_block: SuperBlock::read().await.unwrap(),
            user_infos: User::read().await.unwrap(),
        };
    }
    /// 只从文件系统读出可能更改的root inode信息
    pub async fn update(&mut self) {
        trace!("update SFS");
        self.root_inode = Inode::read(0).await.unwrap();
    }
    ///初始化SFS
    pub async fn init(&mut self) -> Result<(), Error> {
        let sp = SuperBlock::read().await?;
        if sp.valid() {
            trace!("no need to init fs");
            self.read().await;
            return Ok(());
        }
        Err(Error::new(std::io::ErrorKind::Other, "sp broken"))
    }

    /// 打印文件系统的信息
    pub async fn info(&self) -> String {
        let (alloced_inodes, valid_inodes) = count_inodes().await;
        let (alloced, valid) = count_data_blocks().await;
        let (alloced_size, used_unit) = show_unit(alloced * BLOCK_SIZE);
        let (valid_size, valid_unit) = show_unit(valid * BLOCK_SIZE);
        let infos = vec![
            format!("-----------------------\n"),
            format!("{:#?}\n", self.super_block),
            format!("{:#?}\n", self.current_inode),
            format!("[Inode  used] {}\n", alloced_inodes),
            format!("[Inode valid] {}\n", valid_inodes),
            format!("[Disk   used] {}{}\n", alloced_size, used_unit),
            format!("[Disk  valid] {}{}\n", valid_size, valid_unit),
        ];
        infos.concat()
    }

    /// 强制覆盖一份新的FS文件，可以看作是格式化
    pub async fn force_clear(&mut self) {
        info!("init fs");
        create_fs_file().unwrap();

        // 单纯清空缓存，不写入本地文件，用于格式化
        let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
        blk.write().await.block_cache.clear();

        // 创建超级块
        let super_block = SuperBlock::new().await;

        // 创建root_inode
        let root_inode = Inode::new_root().await;

        // 初始化用户信息
        let user_info = User::init().await;

        // 更新缓存
        blk.write().await.sync_and_clear_cache().await.unwrap();

        *self = Self {
            current_inode: root_inode.clone(),
            root_inode,
            super_block,
            user_infos: user_info,
        };
    }

    /// 重置超级块
    pub async fn reset_sp(&mut self) {
        let sp = SuperBlock::new().await;
        self.super_block = sp;
    }

    /// 登录
    pub fn sign_in(&mut self, username: &str, password: &str) -> Result<(), Error> {
        self.user_infos.sign_in(username, password)
    }

    /// 注册
    pub async fn sign_up(&mut self, username: &str, password: &str) -> Result<(), Error> {
        self.user_infos.sign_up(username, password).await
    }

    /// root态下获取所有用户的信息
    pub fn get_users_info(&self, gid: u16) -> Result<UserInfo, Error> {
        if gid != 0 {
            Err(Error::new(
                std::io::ErrorKind::PermissionDenied,
                "not in root",
            ))
        } else {
            Ok(self.user_infos.0.clone())
        }
    }

    /// 根据uid获取用户名
    pub fn get_username(&self, uid: u16) -> Result<String, Error> {
        self.user_infos.get_user_name(uid)
    }

    /// 根据用户名获取id组
    pub fn get_user_ids(&self, username: &str) -> Result<UserIdGroup, Error> {
        let info = self.user_infos.0.get(username).ok_or(Error::new(
            std::io::ErrorKind::NotFound,
            format!("no such user: {}", username),
        ))?;
        Ok(info.1.clone())
    }

    /// 根据用户名获取gid
    pub fn get_user_gid(&self, username: &str) -> Result<u16, Error> {
        Ok(self.get_user_ids(username)?.gid)
    }
}

/// 检查位图对应的区域是否出错
pub async fn check_bitmaps_and_fix() -> Result<(), Error> {
    inode::check_inodes_and_fix().await?;
    block::check_data_and_fix().await
}

/// 创建100MB空文件
pub fn create_fs_file() -> Result<(), Error> {
    File::create(FS_FILE_NAME)?.write_all(&[0u8; FS_SIZE])
}

// 全局变量，管理各种信息
lazy_static! {
    pub static ref SFS: Arc<RwLock<SimpleFileSystem>> =
        Arc::new(RwLock::new(SimpleFileSystem::default()));
}

pub fn show_unit(size: usize) -> (f32, String) {
    match size {
        0..=1023 => (size as f32, "B".to_string()),
        1024..=1048575 => (size as f32 / 1024.0, "KiB".to_string()),
        _ => (size as f32 / (1024.0 * 1024.0), "MiB".to_string()),
    }
}
