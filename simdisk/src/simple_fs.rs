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
    block::BLOCK_CACHE_MANAGER,
    fs_constants::*,
    inode::Inode,
    super_block::SuperBlock,
    user::{User, UserInfo},
};

#[allow(unused)]
#[derive(Default)]
pub struct SampleFileSystem {
    pub root_inode: Inode,
    pub super_block: SuperBlock,
    pub current_inode: Inode,
    pub cwd: String,
    pub user_infos: User,
    pub current_user: UserInfo,
}

impl SampleFileSystem {
    /// 从文件系统中读出相关信息
    pub async fn read(&mut self) {
        trace!("read SFS");
        let root_inode = Inode::read(0).await.unwrap();
        *self = Self {
            current_inode: root_inode.clone(),
            root_inode,
            super_block: SuperBlock::read().await.unwrap(),
            cwd: String::from("~"),
            user_infos: User::read().await.unwrap(),
            current_user: UserInfo::default(),
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
        Err(Error::new(std::io::ErrorKind::Other, ""))
    }

    /// 打印文件系统的信息
    pub async fn info(&self) -> String {
        let (alloced_inodes, valid_inodes) = count_inodes().await;
        let (alloced, valid) = count_data_blocks().await;
        let infos = vec![
            format!("-----------------------\n"),
            format!("{:#?}\n", self.super_block),
            format!("{:#?}\n", self.current_inode),
            format!("[Inode  used] {}\n", alloced_inodes),
            format!("[Inode valid] {}\n", valid_inodes),
            format!("[Disk   used] {} KB\n", alloced),
            format!("[Disk  valid] {} KB\n", valid),
        ];
        infos.concat()
    }

    /// 强制覆盖一份新的FS文件，可以看作是格式化
    pub async fn force_clear(&mut self) {
        info!("init fs");
        // 创建超级块
        let super_block = SuperBlock::new().await;

        // 创建root_inode
        let root_inode = Inode::new_root().await;

        // 初始化用户信息
        let user_info = User::init().await;

        let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
        let mut w = blk.write().await;
        w.sync_and_clear_cache().await.unwrap();

        *self = Self {
            current_inode: root_inode.clone(),
            root_inode,
            super_block,
            cwd: "~".to_string(),
            user_infos: user_info,
            current_user: UserInfo::default(),
        }
    }

    // 重置超级块
    pub async fn check(&mut self) {
        let sp = SuperBlock::new().await;
        self.super_block = sp;
    }

    pub fn sign_in(&mut self, username: &str, password: &str) -> Result<(), Error> {
        let info = self.user_infos.sign_in(username, password)?;
        self.current_user = info;
        Ok(())
    }

    pub async fn sign_up(&mut self, username: &str, password: &str) -> Result<(), Error> {
        self.user_infos.sign_up(username, password).await
    }
}

pub fn create_fs_file() {
    // 创建100MB空文件
    let mut fs_file = File::create(FS_FILE_NAME).expect("cannot create fs file");
    fs_file
        .write_all(&[0u8; FS_SIZE])
        .expect("cannot init fs file");
}

lazy_static! {
    pub static ref SFS: Arc<RwLock<SampleFileSystem>> =
        Arc::new(RwLock::new(SampleFileSystem::default()));
}
