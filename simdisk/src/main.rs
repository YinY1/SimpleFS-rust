use std::sync::Arc;

use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use block::sync_all_block_cache;
use inode::FileMode;
use shell::*;
use simple_fs::SFS;

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
            let mut cmd_buffer;
            let mut is_login = false;
            loop {
                if !is_login {
                    // 0.(1/2).1 等待client 发送信息
                    cmd_buffer = [0; SOCKET_BUFFER_SIZE];
                    let n = match socket.read(&mut cmd_buffer).await {
                        Ok(n) => n,
                        Err(e) => {
                            error!("failed to read from socket; err = {:?}", e);
                            return;
                        }
                    };
                    let response = String::from_utf8_lossy(&cmd_buffer[..n]);
                    let res_vec: Vec<&str> = response.lines().collect();
                    //  0.(1/2).2 验证信息并回信
                    match res_vec[0].trim() {
                        "login" => {
                            if login(&res_vec[1..], &mut socket).await.is_err() {
                                continue;
                            }
                            is_login = true;
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

                // 2.1 接受client的"cwd + 指令"
                cmd_buffer = [0; SOCKET_BUFFER_SIZE];
                let n = match socket.read(&mut cmd_buffer).await {
                    Ok(n) if n == 0 => return,
                    Ok(n) => n,
                    Err(e) => {
                        error!("failed to read from socket; err = {:?}", e);
                        return;
                    }
                };
                let cmd = String::from_utf8_lossy(&cmd_buffer[..n]).replace('\0', "");
                let command = cmd.trim();
                if command == EXIT_MSG {
                    info!("socket {:?} exit", addr);
                    sync_all_block_cache().await.unwrap();
                    return;
                } else if command == EMPTY_INPUT {
                    continue;
                }
                // args[0]为username args[1]为cwd
                let args: Vec<&str> = command.split_whitespace().collect();

                if args[0] == "root" && args[2] == "formatting" {
                    is_login = false;
                }

                let start = tokio::time::Instant::now();
                // 2.2 传输命令执行后的信息
                let msg = match do_command(args, &mut socket).await {
                    Ok(result) => result,
                    Err(err) => {
                        error!("send err back to socket: {:?}, err= {}", addr, err);
                        Some([ERROR_MESSAGE_PREFIX, &err.to_string()].concat())
                    }
                };
                // 2.3 如果有信息要传输
                if let Some(msg) = msg {
                    // 2.3.1 通知对方准备接受内容，等待地址
                    socket.write_all(RECEIVE_CONTENTS.as_bytes()).await.unwrap();
                    // 2.3.2 接受地址
                    cmd_buffer = [0; SOCKET_BUFFER_SIZE];
                    let n = match socket.read(&mut cmd_buffer).await {
                        Ok(n) if n == 0 => return,
                        Ok(n) => n,
                        Err(e) => {
                            error!("failed to read from socket; err = {:?}", e);
                            return;
                        }
                    };
                    let addr = String::from_utf8_lossy(&cmd_buffer[..n]);
                    info!("sending contents through {}", addr);
                    // 2.3.3 发送内容
                    if let Err(e) = send_content(msg, &addr).await {
                        error!("{}", e);
                        return;
                    }
                }

                // 4 宣告结束
                let duration = start.elapsed();
                info!("cmd finished in {:?}", duration);
                socket.write_all(COMMAND_FINISHED.as_bytes()).await.unwrap();
            }
        });
    }
}

async fn do_command(
    args: Vec<&str>,
    socket: &mut TcpStream,
) -> Result<Option<String>, std::io::Error> {
    info!(
        "received args: '{:?}' from socket: {:?}",
        args,
        socket.peer_addr().unwrap()
    );
    let username = args[0];
    let cwd = args[1];
    let commands: Vec<String> = args[2..]
        .iter()
        .map(|&arg| arg.replace('\0', "").trim().to_string())
        .collect();

    if commands[0].as_str() == "dir" {
        if commands.last().unwrap() == "/s" {
            match commands.len() {
                2 => syscall::ls(username, cwd, true).await,
                3 => {
                    let target_path = get_absolute_path(cwd, &commands[1]);
                    syscall::ls(username, &target_path, true).await
                }
                _ => Err(error_arg()),
            }
        } else {
            match commands.len() {
                1 => syscall::ls(username, cwd, false).await,
                2 => {
                    let target_path = get_absolute_path(cwd, &commands[1]);
                    syscall::ls(username, &target_path, false).await
                }
                _ => Err(error_arg()),
            }
        }
    } else {
        match commands.len() {
            1 => match commands[0].as_str() {
                "info" => syscall::info().await,
                "check" => syscall::check().await.map(|_| None),
                "users" => syscall::get_users_info(username).await,
                "formatting" => syscall::formatting(username).await.map(|_| None),
                _ => Err(error_arg()),
            },
            2 => {
                let absolut_path = get_absolute_path(cwd, &commands[1]);
                match commands[0].as_str() {
                    "cd" => syscall::cd(&absolut_path).await.map(|_| None),
                    "md" => syscall::mkdir(username, &absolut_path).await.map(|_| None),
                    // 对于rd 要等待client确认是否删除
                    "rd" => syscall::rmdir(username, &absolut_path, socket)
                        .await
                        .map(|_| None),
                    // 对于newfile 需要输入文件内容，要等待client传输内容
                    "newfile" => syscall::new_file(username, &absolut_path, FileMode::RDWR, socket)
                        .await
                        .map(|_| None),
                    "cat" => syscall::cat(&absolut_path).await,
                    "del" => syscall::del(username, &absolut_path).await.map(|_| None),
                    _ => Err(error_arg()),
                }
            }
            3 => match commands[0].as_str() {
                "copy" => {
                    let target_path = get_absolute_path(cwd, &commands[2]);
                    syscall::copy(username, commands[1].as_str(), &target_path, socket)
                        .await
                        .map(|_| None)
                }
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
    std::io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid args, input 'help' to see commands",
    )
}

fn get_absolute_path(cwd: &str, path: &str) -> String {
    if path.starts_with('~') {
        // 绝对路径
        path.to_string()
    } else {
        // 相对路径
        [cwd, "/", path].concat()
    }
}
