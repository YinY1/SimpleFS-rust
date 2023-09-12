use std::io::{self, Write};

use simple_fs::SFS;

use crate::inode::FileMode;

mod bitmap;
mod block;
mod dirent;
mod file;
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
fn basic_bash() {
    loop {
        print!("\n{}\n$ ", SFS.lock().unwrap().cwd);
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();

        let input = input.trim();

        if input.is_empty() {
            continue;
        }
        if input == "EXIT" {
            return;
        }
        let args: Vec<&str> = input.split_whitespace().collect();
        if args[0] == "ls" {
            if args.last().unwrap() == &"/s" {
                match args.len() {
                    2 => syscall::ls(true),
                    3 => syscall::ls_dir(args[1], true),
                    _ => println!("invalid args"),
                }
            } else {
                match args.len() {
                    1 => syscall::ls(false),
                    2 => syscall::ls(true),
                    _ => println!("invalid args"),
                }
            }
        } else {
            match args.len() {
                1 => match args[0] {
                    "info" => syscall::info(),
                    "check" => syscall::check(),
                    _ => println!("invalid args"),
                },
                2 => {
                    let name = args[1];
                    match args[0] {
                        "cd" => syscall::cd(name),
                        "md" => syscall::mkdir(name),
                        "rd" => syscall::rmdir(name),
                        "newfile" => syscall::new_file(name, FileMode::RDWR),
                        "cat" => syscall::cat(name),
                        "del" => syscall::del(name),
                        _ => println!("invalid args"),
                    }
                }
                3 => match args[0] {
                    "copy" => syscall::copy(args[1], args[2]),
                    _ => println!("invalid args"),
                },
                _ => println!("invalid args"),
            }
        }
    }
}
