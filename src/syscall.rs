use log::error;

use crate::{block::sync_all_block_cache, dirent, simple_fs::SFS};

/// 打印
#[allow(unused)]
pub fn info() {
    SFS.lock().info();
}

#[allow(unused)]
pub fn ls() {
    SFS.lock().current_inode.ls();
}

#[allow(unused)]
pub fn mkdir(name: &str) {
    if dirent::mkdir(name, &mut SFS.lock().current_inode).is_none() {
        error!("error in mkdir");
    } else {
        sync_all_block_cache();
    }
}

#[allow(unused)]
pub fn rmdir(name: &str) {
    if dirent::rmdir(name, &mut SFS.lock().current_inode).is_none() {
        error!("error in rmdir");
    } else {
        sync_all_block_cache();
    }
}

#[allow(unused)]
pub fn cd(name: &str) {
    if dirent::cd(name).is_none() {
        error!("error in cd");
    }
}
