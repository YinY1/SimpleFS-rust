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

    loop {
        let mut stream_buffer = [0; 1024];

        // 0. 发送信息请求bash
        let msg = BASH_REQUEST;
        stream.write_all(msg.as_bytes()).await?;

        // 1. 获取cwd
        let len = stream.read(&mut stream_buffer).await?;
        if len == 0 {
            error!("error reading answer from server");
            return Err(Error::new(ErrorKind::NotConnected, ""));
        }

        let recv_cwd = String::from_utf8_lossy(&stream_buffer);

        println!("{}", recv_cwd.replace('\0', ""));
        print!("$ ");
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

        // 2.1 将指令发给server
        stream.write_all(input.as_bytes()).await?;

        // 2.3 读取返回信息，如果是需要继续输入信息的，则回复，否则不回复
        stream_buffer = [0; 1024];
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
                // 2. ex1.2 将得到的文件内容发送给server
                stream.write_all(inputs.as_bytes()).await?;
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
            // 正常返回文件内容（可能是正常信息，也可能是错误信息
            COMMAND_FINISHED => continue,
            _ => {
                println!("{}", msg);
                continue;
            }
        };
        // 3. 等待server应答
        stream_buffer = [0; 1024];
        let n = stream.read(&mut stream_buffer).await?;
        if n == 0 {
            error!("error reading answer from server");
            return Err(Error::new(ErrorKind::NotConnected, ""));
        }
        let msg = String::from_utf8_lossy(&stream_buffer).replace('\0', "");
        if msg.trim() != COMMAND_FINISHED {
            println!("{}", msg);
        }
    }
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
    debug!("get intputs: --->[{}]<---", inputs);
    Ok(inputs)
}
