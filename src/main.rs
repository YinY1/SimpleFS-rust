use simple_fs::SFS;

mod bitmap;
mod block;
mod dirent;
mod inode;
mod simple_fs;
mod super_block;
mod syscall;

#[macro_use]
extern crate lazy_static;

fn main() {
    env_logger::init();
    syscall::info();
    syscall::ls();
    mkdir_test();
}

#[allow(unused)]
fn mkdir_test() {
    syscall::mkdir("test").unwrap();
    syscall::info();
    syscall::ls();
}
#[allow(unused)]
fn force_init() {
    SFS.lock().force_clear();
}
