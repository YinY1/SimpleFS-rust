mod bitmap;
mod block;
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
}
