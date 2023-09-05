mod inode;
mod simple_fs;

use simple_fs::SuperBlock;

fn main() {
    env_logger::init();
    simple_fs::init();

    sp_test();
}

#[allow(unused)]
fn sp_test() {
    let sp = SuperBlock::read().unwrap();
    println!("{:?}", sp);
}
