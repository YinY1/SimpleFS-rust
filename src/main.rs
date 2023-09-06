use simple_fs::SampleFileSystem;

mod bitmap;
mod block;
mod inode;
mod simple_fs;
mod super_block;

#[macro_use]
extern crate lazy_static;
fn main() {
    env_logger::init();
    let fs = SampleFileSystem::init();
    fs.info();
}
