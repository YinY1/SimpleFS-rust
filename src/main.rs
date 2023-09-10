use std::io::{self, Write};

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
    basic_bash();
}

#[allow(unused)]
fn mkdir_test() {
    syscall::mkdir("test");
    syscall::info();
    syscall::ls();
}

#[allow(unused)]
fn rmdir_test() {
    syscall::rmdir("test");
    syscall::info();
    syscall::ls();
}

#[allow(unused)]
fn force_init() {
    SFS.lock().force_clear();
}

#[allow(unused)]
fn basic_bash() {
    loop {
        print!("\n{}\n$ ", SFS.lock().cwd);
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();

        let input = input.trim();

        if input.is_empty() {
            continue;
        }
        if input == "quit" {
            return;
        }
        let args: Vec<&str> = input.split_whitespace().collect();
        match args.len() {
            1 => match args[0] {
                "ls" => syscall::ls(),
                "info" => syscall::info(),
                _ => println!("invalid args"),
            },
            2 => {
                let name = args[1];
                match args[0] {
                    "cd" => syscall::cd(name),
                    "mkdir" => syscall::mkdir(name),
                    "rmdir" => syscall::rmdir(name),
                    _ => println!("invalid args"),
                }
            }
            _ => println!("invalid args"),
        }
    }
}
