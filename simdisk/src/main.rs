use std::sync::Arc;

use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use simple_fs::SFS;

use crate::block::sync_all_block_cache;
use crate::inode::FileMode;
use crate::simple_fs::create_fs_file;
use shell::*;

mod bitmap;
mod block;
mod dirent;
mod file;
mod fs_constants;
mod inode;
mod simple_fs;
mod super_block;
mod syscall;
mod user;

#[macro_use]
extern crate lazy_static;
extern crate pretty_env_logger;
#[macro_use]
extern crate log;

#[tokio::main]
async fn main() -> io::Result<()> {
    pretty_env_logger::formatted_builder()
        .filter_level(log::LevelFilter::Info)
        .init();

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
        info!("connected to {:?}", addr);
        // spawn一个线程
        tokio::spawn(async move {
            let mut buffer;
            let mut is_login = false;
            loop {
                buffer = [0; 1024];
                // 0.0 接受bash ok请求
                let n = match socket.read(&mut buffer).await {
                    Ok(n) if n == 0 => return,
                    Ok(n) => n,
                    Err(e) => {
                        error!("failed to read from socket; err = {:?}", e);
                        return;
                    }
                };
                let bash = String::from_utf8_lossy(&buffer[..n]);
                if bash.replace('\0', "").trim() != BASH_REQUEST {
                    error!("wrong request for cwd, arg={}", bash);
                    return;
                }

                if !is_login {
                    // 0.0.1 请求登录
                    info!("ask user login or regist");
                    if let Err(e) = socket.write_all(LOGIN_REQUEST.as_bytes()).await {
                        error!("failed to write to socket; err = {:?}", e);
                        return;
                    }
                    // 0.(1/2).1 等待client 发送信息
                    buffer = [0; 1024];
                    let n = match socket.read(&mut buffer).await {
                        Ok(n) => n,
                        Err(e) => {
                            error!("failed to read from socket; err = {:?}", e);
                            return;
                        }
                    };
                    let response = String::from_utf8_lossy(&buffer[..n]);
                    let res_vec: Vec<&str> = response.split_whitespace().collect();
                    //  0.(1/2).2 验证信息并回信
                    match res_vec[0].trim() {
                        "login" => {
                            if login(&res_vec[1..], &mut socket).await.is_err() {
                                continue;
                            }
                            is_login = true;
                            // 1.0 读取cwd请求
                            buffer = [0; 1024];
                            let len = socket.read(&mut buffer).await.unwrap();
                            if len == 0
                                || String::from_utf8_lossy(&buffer[..len]).trim() != CWD_REQUEST
                            {
                                error!("error reading answer from client");
                                return;
                            }
                        }
                        "regist" => {
                            regist(&res_vec[1..], &mut socket).await;
                            continue;
                        }
                        _ => {
                            error!("invalid {}", res_vec[0]);
                            return;
                        }
                    }
                }

                // 1. 将cwd发送给client
                let fs = Arc::clone(&SFS);
                let cwd = fs.read().await.cwd.clone();
                drop(fs);
                if let Err(e) = socket.write_all(cwd.as_bytes()).await {
                    error!("failed to write to socket; err = {:?}", e);
                    return;
                }

                // 2.1 接受client的指令
                buffer = [0; 1024];
                let _ = match socket.read(&mut buffer).await {
                    Ok(n) if n == 0 => return,
                    Ok(n) => n,
                    Err(e) => {
                        error!("failed to read from socket; err = {:?}", e);
                        return;
                    }
                };
                let cmd = String::from_utf8_lossy(&buffer).replace('\0', "");
                let command = cmd.trim();
                if command == EXIT_MSG {
                    info!("socket {:?} exit", addr);
                    sync_all_block_cache().await.unwrap();
                    return;
                } else if command == EMPTY_INPUT {
                    continue;
                }
                let args: Vec<&str> = command.split_whitespace().collect();

                // 2.2 传输命令执行后的信息
                let _ = match do_command(args, &mut socket).await {
                    Ok(result) => match result {
                        // 3.1 对于需要返回信息的command（ls等）写回给client
                        Some(output) => {
                            info!("cmd successfully get output");
                            socket.write_all(output.as_bytes()).await
                        }
                        // 3.2 不需要返回信息的command（cd等）写回ok信息给client
                        None => {
                            info!("cmd finished");
                            socket.write_all(COMMAND_FINISHED.as_bytes()).await
                        }
                    },
                    // 3.3 命令执行出错的写回err
                    Err(err) => {
                        error!("send err back to socket: {:?}, err= {}", addr, err);
                        socket.write_all(err.to_string().as_bytes()).await
                    }
                };
            }
        });
    }
}

async fn do_command(
    args: Vec<&str>,
    socket: &mut TcpStream,
) -> Result<Option<String>, std::io::Error> {
    let args: Vec<String> = args
        .iter()
        .map(|&arg| arg.replace('\0', "").trim().to_string())
        .collect();
    info!(
        "received args: '{:?}' from socket: {:?}",
        args,
        socket.peer_addr().unwrap()
    );
    if args[0].as_str() == "dir" {
        if args.last().unwrap() == "/s" {
            match args.len() {
                2 => syscall::ls(true).await,
                3 => syscall::ls_dir(&args[1], true).await,
                _ => Err(error_arg()),
            }
        } else {
            match args.len() {
                1 => syscall::ls(false).await,
                2 => syscall::ls_dir(&args[1], false).await,
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

async fn login(user: &[&str], socket: &mut TcpStream) -> Result<(), ()> {
    let fs = Arc::clone(&SFS);
    let mut fs_write_lock = fs.write().await;
    if let Err(e) = fs_write_lock.sign_in(user[0], user[1]) {
        // 回信client登录失败
        socket.write_all(e.to_string().as_bytes()).await.unwrap();
        return Err(());
    }
    // 0.1.2 回信成功
    socket.write_all(LOGIN_SUCCESS.as_bytes()).await.unwrap();
    Ok(())
}

async fn regist(user: &[&str], socket: &mut TcpStream) {
    let fs = Arc::clone(&SFS);
    let mut fs_write_lock = fs.write().await;
    if let Err(e) = fs_write_lock.sign_up(user[0], user[1]).await {
        // 回信client注册失败
        socket.write_all(e.to_string().as_bytes()).await.unwrap();
        return;
    }
    info!("user: {} signed up", user[0]);
    // 0.2.2 回信成功
    socket.write_all(REGIST_SUCCESS.as_bytes()).await.unwrap();
}
fn error_arg() -> std::io::Error {
    std::io::Error::new(io::ErrorKind::InvalidInput, "invalid args")
}
