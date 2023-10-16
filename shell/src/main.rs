use std::io::{Error, Write};

use utils::*;
use tokio::io::{self, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, ErrorKind, Stdin};
use tokio::net::{TcpListener, TcpStream};

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
    let mut cwd = "~".to_string();

    loop {
        if !is_login {
            // 0.(1/2).1 选择注册还是登录
            info!("select: \n[1]sign In\n[2]sign Up");
            let mut choice = String::new();
            io_reader.read_line(&mut choice).await?;
            match choice.to_lowercase().trim() {
                "sign in" | "1" | "i" => {
                    // 向server发送登录信息
                    if login(&mut username, &mut io_reader, &mut stream)
                        .await
                        .is_err()
                    {
                        continue;
                    };
                    is_login = true;
                }
                "sign up" | "2" | "u" => {
                    // 向server发送注册信息
                    if let Err(e) = regist(&mut io_reader, &mut stream).await {
                        error!("{}", e);
                    }
                    continue;
                }
                _ => {
                    error!("invalid arg");
                    continue;
                }
            }
        }

        println!("{}", cwd);
        print!("({}) $ ", username.trim());
        std::io::stdout().flush()?;

        // 2.0 读取输入指令
        let mut input = String::new();
        io_reader.read_line(&mut input).await?;
        let input = input.trim();
        if input.is_empty() {
            // 输入为空 发送一个特定消息告诉server放弃接下来的读取
            stream.write_all(EMPTY_INPUT.as_bytes()).await?;
            continue;
        }
        match input.to_uppercase().trim() {
            EXIT_MSG => {
                stream.write_all(EXIT_MSG.as_bytes()).await?;
                return Ok(());
            }
            HELP_REQUEST => {
                print_help(&username);
                stream.write_all(EMPTY_INPUT.as_bytes()).await?;
                continue;
            }
            _ => {}
        }

        // 2.1 将username+ cwd +指令发给server
        let cmd = [&username, " ", &cwd, " ", input].concat();
        stream.write_all(cmd.as_bytes()).await?;

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
            input_msg if msg.starts_with(INPUT_FILE_CONTENT) => {
                let inputs = read_file_content(&mut io_reader).await?;
                // 解析端口
                let addr = input_msg.strip_prefix(INPUT_FILE_CONTENT).unwrap();
                // 2. ex1.2 将得到的文件内容通过给定端口发送给server
                send_content(inputs, addr).await?;
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
            // 2.3.1 需要打开文件通道接受内容
            RECEIVE_CONTENTS => {
                // 建立临时socket，端口随机
                let listener = TcpListener::bind("127.0.0.1:0").await?;
                // 2.3.2 将端口写给server
                let addr = listener.local_addr()?;
                stream.write_all(addr.to_string().as_bytes()).await?;
                // 2.3.3 接受内容
                let contents = receive_content(&listener).await?;
                if contents.starts_with(ERROR_MESSAGE_PREFIX) {
                    error!("{}", contents.strip_prefix(ERROR_MESSAGE_PREFIX).unwrap());
                } else {
                    println!("{}", contents);
                }
                // -->跳转到3.
            }
            // 4. 本次指令执行完毕
            COMMAND_FINISHED => {
                if input.starts_with("cd") {
                    // 处理cwd情况
                    deal_with_dir(input, &mut cwd);
                } else if input == "formatting" {
                    // 格式化之后要退出登录
                    is_login = false;
                }
                continue;
            }
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
        // 4 宣告结束，否则打印错误信息
        if msg.trim() != COMMAND_FINISHED {
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

/// 从标准输入读取长内容
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
    Ok(inputs)
}

fn print_help(username: &str) {
    println!("info");
    println!("dir (path) (/s)");
    println!("cd [path]");
    println!("md [path]");
    println!("rd [path]");
    println!("newfile [filename]");
    println!("cat [filename]");
    println!("copy (<host>)[src path] [dst path]");
    println!("check");
    if username == "root" {
        println!("formatting");
        println!("users");
    }
    println!("EXIT");
}

fn deal_with_dir(input: &str, cwd: &mut String) {
    // 在shell本地处理cwd
    let path = input.split_whitespace().collect::<Vec<&str>>()[1];
    //将路径分割为多段
    let mut paths: Vec<&str> = path.split('/').collect();
    if paths[0] == "~" {
        cwd.clear();
        cwd.push('~');
        paths.remove(0);
    }
    // 调整当前目录
    for &path in &paths {
        match path {
            "." => {}
            ".." => {
                let idx = cwd.rfind('/').unwrap();
                cwd.replace_range(idx.., "");
            }
            _ => cwd.push_str(&["/", path].concat()),
        }
    }
}
