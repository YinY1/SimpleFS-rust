mod bitmap;
mod block;
mod inode;
mod simple_fs;
mod super_block;
mod syscall;
mod dirent;

#[macro_use]
extern crate lazy_static;

fn main() {
    env_logger::init();
    syscall::info();
    syscall::ls();
    mkdir_test();
}

#[allow(unused)]
fn mkdir_test(){
    syscall::mkdir("test").unwrap();
    syscall::info();
    syscall::ls();
}
