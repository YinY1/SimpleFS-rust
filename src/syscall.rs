use log::error;

use crate::{block::sync_all_block_cache, dirent, file, inode::FileMode, simple_fs::SFS};

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
    if dirent::make_directory(name, &mut SFS.lock().current_inode).is_none() {
        error!("error in mkdir");
    } else {
        sync_all_block_cache();
    }
}

#[allow(unused)]
pub fn rmdir(name: &str) {
    if dirent::remove_directory(name, &mut SFS.lock().current_inode).is_none() {
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

#[allow(unused)]
pub fn new_file(name: &str, mode: FileMode) {
    if file::create_file(name, mode, &mut SFS.lock().current_inode).is_none() {
        error!("error in newfile");
    } else {
        sync_all_block_cache();
    }
}

#[allow(unused)]
pub fn del(name: &str) {
    if file::remove_file(name, &mut SFS.lock().current_inode).is_none() {
        error!("error in del");
    } else {
        sync_all_block_cache();
    }
}

#[allow(unused)]
pub fn cat(name: &str) {
    match file::open_file(name, &SFS.lock().current_inode) {
        Some(content) => println!("{}", content),
        None => error!("error in cat"),
    }
}

#[allow(unused)]
pub fn check() {
    SFS.lock().check();
}
