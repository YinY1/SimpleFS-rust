use crate::simple_fs::SFS;

pub fn info() {
    SFS.lock().info();
}

pub fn ls() {
    SFS.lock().current_inode.ls();
}
