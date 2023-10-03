use std::io::{Error, Write};

use shell::*;
use tokio::io::{self, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, ErrorKind, Stdin};
use tokio::net::TcpStream;

extern crate pretty_env_logger;
#[macro_use]
extern crate log;

#[tokio::main]
async fn main() -> io::Result<()> {
    pretty_env_logger::formatted_builder()
        .filter_level(log::LevelFilter::Info)
        .init();

    let mut stream = TcpStream::connect(SOCKET_ADDR).await?;
    info!("Connected to server");
    let mut io_reader = io::BufReader::new(io::stdin());
    let mut stream_buffer;
    let mut is_login = false;
    let mut username = String::new();
    loop {
        // 0. 发送信息请求bash
        let msg = BASH_REQUEST;
        stream.write_all(msg.as_bytes()).await?;

        if !is_login {
            // 0.0.1 获取登录请求
            trace!("waiting login request");
            stream_buffer = [0; SOCKET_BUFFER_SIZE];
            let n = stream.read(&mut stream_buffer).await?;
            if n == 0 {
                error!("error reading answer from server");
                return Err(Error::new(ErrorKind::NotConnected, ""));
            }
            let login_request = String::from_utf8_lossy(&stream_buffer[..n]);
            if login_request != LOGIN_REQUEST {
                error!("error login in server");
                return Err(Error::new(ErrorKind::NotConnected, ""));
            }
            // 选择注册还是登录
            info!("select: \n[1]sign In\n[2]sign Up\n[3]EXIT");
            let mut choice = String::new();
            io_reader.read_line(&mut choice).await?;
            match choice.to_lowercase().trim() {
                "sign in" | "1" | "i" => {
                    if login(&mut username, &mut io_reader, &mut stream)
                        .await
                        .is_err()
                    {
                        continue;
                    };
                    is_login = true;
                    // 1.0 请求cwd
                    stream.write_all(CWD_REQUEST.as_bytes()).await?;
                }
                "sign up" | "2" | "u" => {
                    if let Err(e) = regist(&mut io_reader, &mut stream).await {
                        error!("{}", e);
                    }
                    continue;
                }
                "exit" => return Err(Error::new(ErrorKind::ConnectionReset, "")),
                _ => {
                    error!("invalid arg");
                    return Err(Error::new(ErrorKind::InvalidInput, ""));
                }
            }
        }

        // 1. 获取cwd
        stream_buffer = [0; SOCKET_BUFFER_SIZE];
        let len = stream.read(&mut stream_buffer).await?;
        if len == 0 {
            error!("error reading answer from server");
            return Err(Error::new(ErrorKind::NotConnected, ""));
        }

        let recv_cwd = String::from_utf8_lossy(&stream_buffer[..len]);

        println!("{}", recv_cwd.replace('\0', ""));
        print!("({}) $ ", username.trim());
        std::io::stdout().flush()?;

        // 2.0 读取输入
        let mut input = String::new();
        io_reader.read_line(&mut input).await?;
        let input = input.trim();
        if input.is_empty() {
            // 输入为空 发送一个特定消息告诉server放弃接下来的读取
            stream.write_all(EMPTY_INPUT.as_bytes()).await?;
            continue;
        }
        if input.trim() == EXIT_MSG {
            stream.write_all(input.as_bytes()).await?;
            return Ok(());
        }
        if input.to_lowercase().trim() == HELP_REQUEST {
            print_help();
            stream.write_all(EMPTY_INPUT.as_bytes()).await?;
            continue;
        }

        // 2.1 将指令发给server
        stream.write_all(input.as_bytes()).await?;

        // 2.3 读取返回信息，如果是需要继续输入信息的，则回复，否则不回复
        stream_buffer = [0; SOCKET_BUFFER_SIZE];
        let n = stream.read(&mut stream_buffer).await?;
        if n == 0 {
            error!("error reading answer from server");
            return Err(Error::new(ErrorKind::NotConnected, ""));
        }
        let msg = String::from_utf8_lossy(&stream_buffer).replace('\0', "");
        match msg.trim() {
            // 2. ex1.1 需要输入文件内容
            INPUT_FILE_CONTENT => {
                let inputs = read_file_content(&mut io_reader).await?;
                // 2. ex1.2 将得到的文件内容通过8081发送给server
                send_content(inputs).await?;
            }
            // 需要确认是否继续执行
            COMMAND_CONFIRM => {
                // 2.ex2 将确认指令回复给server
                println!("diretory is not empty, continue to remove? [y/n]");
                let mut answer = String::new();
                let n = io_reader.read_line(&mut answer).await?;
                if n == 0 {
                    stream.write_all("n".as_bytes()).await?;
                    continue;
                }
                stream.write_all(answer.as_bytes()).await?;
            }
            // 2.3 需要打开文件通道接受内容
            RECEIVE_CONTENTS => {
                let contents = receive_content().await?;
                println!("{}", contents);
                // -->跳转到3.
            }
            // 4. 本次指令执行完毕
            COMMAND_FINISHED => continue,
            _ => {
                panic!("{}", msg);
            }
        };
        // 3. 等待server应答
        stream_buffer = [0; SOCKET_BUFFER_SIZE];
        let n = stream.read(&mut stream_buffer).await?;
        if n == 0 {
            error!("error reading answer from server");
            return Err(Error::new(ErrorKind::NotConnected, ""));
        }
        let msg = String::from_utf8_lossy(&stream_buffer).replace('\0', "");
        if msg.trim() != COMMAND_FINISHED {
            // 4 宣告结束
            println!("{}", msg);
        }
    }
}

async fn login(
    username: &mut String,
    io_reader: &mut BufReader<Stdin>,
    stream: &mut TcpStream,
) -> io::Result<()> {
    // 输入用户信息
    info!("enter username");
    username.clear();
    io_reader.read_line(username).await?;
    info!("enter password");
    let mut password = String::new();
    io_reader.read_line(&mut password).await?;

    //  0.1.1 发送登录信息
    stream
        .write_all(["login\n", username, &password].concat().as_bytes())
        .await?;
    // 0.1.2 接受回传信息
    let mut stream_buffer = [0; SOCKET_BUFFER_SIZE];
    let n = stream.read(&mut stream_buffer).await?;
    if n == 0 {
        error!("error reading answer from server");
        return Err(Error::new(ErrorKind::NotConnected, ""));
    }
    let login_response = String::from_utf8_lossy(&stream_buffer[..n]);
    if login_response != LOGIN_SUCCESS {
        error!("login failed, {}", login_response);
        return Err(Error::new(ErrorKind::PermissionDenied, login_response));
    }
    Ok(())
}

async fn regist(io_reader: &mut BufReader<Stdin>, stream: &mut TcpStream) -> io::Result<()> {
    // 输入用户信息
    info!("sign up user");
    let mut username = String::new();
    io_reader.read_line(&mut username).await?;
    let mut password = String::new();
    io_reader.read_line(&mut password).await?;

    //  0.2.1 发送注册信息
    stream
        .write_all(["regist\n", &username, &password].concat().as_bytes())
        .await?;
    // 0.2.2 接受回传信息
    let mut stream_buffer = [0; SOCKET_BUFFER_SIZE];
    let n = stream.read(&mut stream_buffer).await?;
    if n == 0 {
        error!("error reading answer from server");
        return Err(Error::new(ErrorKind::NotConnected, ""));
    }
    let regist_response = String::from_utf8_lossy(&stream_buffer[..n]);
    if regist_response != REGIST_SUCCESS {
        error!("regist failed");
        return Err(Error::new(ErrorKind::PermissionDenied, regist_response));
    }
    Ok(())
}

async fn read_file_content(io_reader: &mut BufReader<Stdin>) -> io::Result<String> {
    let mut line = String::new();
    let mut inputs = String::new();
    while let Ok(bytes_read) = io_reader.read_line(&mut line).await {
        if bytes_read == 0 {
            debug!("input over");
            break; // 读取完毕，输入结束
        }
        inputs.push_str(&line);
        line.clear();
    }
    info!("get intputs: --->[{}]<---", inputs);
    Ok(inputs)
}

fn print_help() {
    println!("info");
    println!("dir (path) (/s)");
    println!("cd [path]");
    println!("md [path]");
    println!("rd [path]");
    println!("newfile [filename]");
    println!("cat [filename]");
    println!("copy (<host>)[src path] [dst path]");
    println!("check");
    println!("EXIT");
}
