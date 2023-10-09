use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Error};

use crate::{
    block::{deserialize, get_block_buffer, write_block},
    fs_constants::{BLOCK_SIZE, USER_START_BYTE},
};

#[derive(Serialize, Deserialize, Default, Hash, Clone, Debug)]
pub struct UserIdGroup {
    pub gid: u16,
    pub uid: u16,
}

// map{username: (password, (gid,uid))}
pub type UserInfo = HashMap<String, (String, UserIdGroup)>;

#[derive(Serialize, Deserialize, Default)]
pub struct User(pub UserInfo);

impl User {
    /// 初始化创建root用户
    pub async fn init() -> Self {
        let mut s = Self(HashMap::new());
        let info = UserIdGroup { gid: 0, uid: 0 };
        s.0.insert("root".to_owned(), ("admin".to_owned(), info));
        s.cache().await;
        s
    }

    /// 从磁盘中读取用户信息
    pub async fn read() -> Result<Self, Error> {
        let buffer = get_block_buffer(0, USER_START_BYTE, BLOCK_SIZE).await?;
        deserialize(&buffer)
    }

    /// 注册用户
    pub async fn sign_up(&mut self, username: &str, password: &str) -> Result<(), Error> {
        if self.0.contains_key(username) {
            return Err(Error::new(
                std::io::ErrorKind::PermissionDenied,
                "user exists",
            ));
        }
        let info = UserIdGroup {
            gid: 1,
            uid: self.get_user_num() as u16 + 1,
        };
        self.0
            .insert(username.to_owned(), (password.to_owned(), info));
        self.cache().await;
        Ok(())
    }

    /// 登录
    pub fn sign_in(&self, username: &str, password: &str) -> Result<(), Error> {
        match self.0.get(username) {
            Some(info) => {
                if info.0 == password {
                    return Ok(());
                }
                Err(Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "incorrect password",
                ))
            }
            None => Err(Error::new(
                std::io::ErrorKind::PermissionDenied,
                "user not exists",
            )),
        }
    }

    /// 根据uid得到用户名
    pub fn get_user_name(&self, uid: u16) -> Result<String, Error> {
        match self.0.iter().find_map(|(username, (_, ids))| {
            if ids.uid == uid {
                Some(username.to_string())
            } else {
                None
            }
        }) {
            Some(username) => Ok(username),
            None => Err(Error::new(std::io::ErrorKind::NotFound, "user not exists")),
        }
    }

    fn get_user_num(&self) -> usize {
        self.0.len() - 1
    }

    async fn cache(&self) {
        write_block(self, 0, USER_START_BYTE).await.unwrap();
    }
}

/// 判断当前uid是否有权限修改other uid创建的文件
pub fn able_to_modify(this: u16, other: u16) -> bool {
    this <= other
}
