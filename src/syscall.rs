use crate::{simple_fs::SFS, dirent, block::sync_all_block_cache};

pub fn info() {
    SFS.lock().info();
}

pub fn ls() {
    SFS.lock().current_inode.ls();
}

pub fn mkdir(name:&str) -> Option<()> {
    dirent::mkdir(name,&mut SFS.lock().current_inode)?;
    sync_all_block_cache();
    Some(())
}