use std::sync::Arc;

use log::info;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use simple_fs::SFS;

use crate::block::sync_all_block_cache;
use crate::inode::FileMode;
use crate::simple_fs::create_fs_file;

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

const SOCKET_ADDR: &str = "127.0.0.1:8080";

#[tokio::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let fs = Arc::clone(&SFS);
    let mut w = fs.write().await;
    if w.init().await.is_err() {
        create_fs_file();
        w.force_clear().await;
        info!("SFS init successfully");
    };
    drop(w);

    let listener = TcpListener::bind(SOCKET_ADDR).await?;
    info!("server listening to {}", SOCKET_ADDR);

    loop {
        let (mut socket, addr) = listener.accept().await?;

        // spawn一个线程
        tokio::spawn(async move {
            let mut buffer = [0; 1024];
            loop {
                // 0. 接受bash ok请求
                let _ = match socket.read(&mut buffer).await {
                    Ok(n) if n == 0 => return,
                    Ok(n) => n,
                    Err(e) => {
                        eprintln!("failed to read from socket; err = {:?}", e);
                        return;
                    }
                };
                let bash = String::from_utf8_lossy(&buffer);
                if bash.trim().replace('\0', "") == "bash ok" {
                    let fs = Arc::clone(&SFS);
                    let cwd = fs.read().await.cwd.clone();
                    // 1. 将cwd发送给client
                    if let Err(e) = socket.write_all(cwd.as_bytes()).await {
                        eprintln!("failed to write to socket; err = {:?}", e);
                        return;
                    }
                } else {
                    eprintln!("wrong request for cwd");
                    return;
                }

                // 2.1 接受client的指令
                buffer = [0; 1024];
                let _ = match socket.read(&mut buffer).await {
                    Ok(n) if n == 0 => return,
                    Ok(n) => n,
                    Err(e) => {
                        eprintln!("failed to read from socket; err = {:?}", e);
                        return;
                    }
                };
                let command = String::from_utf8_lossy(&buffer);
                if command.trim().replace('\0', "") == "EXIT" {
                    println!("socket {:?} exit", addr);
                    let _ = sync_all_block_cache().await;
                    return;
                }
                let args: Vec<&str> = command.split_whitespace().collect();

                // 2.2 传输命令执行后的信息
                let _ = match do_command(args, &mut socket).await {
                    Ok(result) => match result {
                        // 2.3.1 对于需要返回信息的command（ls等）写回给client
                        Some(output) => socket.write_all(output.as_bytes()).await,
                        // 2.3.2 不需要返回信息的command（cd等）写回ok信息给client
                        None => socket.write_all("command ok.".as_bytes()).await,
                    },
                    // 2.3.3 命令执行出错的写回err
                    Err(err) => socket.write_all(err.to_string().as_bytes()).await,
                };
            }
        });
    }
}

#[allow(unused)]
async fn do_command(
    mut args: Vec<&str>,
    socket: &mut TcpStream,
) -> Result<Option<String>, std::io::Error> {
    let args: Vec<String> = args.iter().map(|&arg| arg.replace('\0', "")).collect();
    if args[0].as_str() == "ls" {
        if args.last().unwrap() == "/s" {
            match args.len() {
                2 => syscall::ls(true).await,
                3 => syscall::ls_dir(&args[1], true).await,
                _ => Err(error_arg()),
            }
        } else {
            match args.len() {
                1 => syscall::ls(false).await,
                2 => syscall::ls(true).await,
                _ => Err(error_arg()),
            }
        }
    } else {
        match args.len() {
            1 => match args[0].as_str() {
                "info" => syscall::info().await,
                "check" => syscall::check().await.map(|_| None),
                _ => Err(error_arg()),
            },
            2 => {
                let name = args[1].as_str();
                match args[0].as_str() {
                    "cd" => syscall::cd(name).await.map(|_| None),
                    "md" => syscall::mkdir(name).await.map(|_| None),
                    // 对于rd 要等待client确认是否删除
                    "rd" => syscall::rmdir(name, socket).await.map(|_| None),
                    // 对于newfile 需要输入文件内容，要等待client传输内容
                    "newfile" => syscall::new_file(name, FileMode::RDWR, socket)
                        .await
                        .map(|_| None),
                    "cat" => syscall::cat(name).await,
                    "del" => syscall::del(name).await.map(|_| None),
                    _ => Err(error_arg()),
                }
            }
            3 => match args[0].as_str() {
                "copy" => syscall::copy(args[1].as_str(), args[2].as_str(), socket)
                    .await
                    .map(|_| None),
                _ => Err(error_arg()),
            },
            _ => Err(error_arg()),
        }
    }
}

fn error_arg() -> std::io::Error {
    std::io::Error::new(io::ErrorKind::InvalidInput, "invalid args")
}
