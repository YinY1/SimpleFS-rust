use std::io::Error;

use tokio::io::{self, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, ErrorKind, Stdin};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() -> io::Result<()> {
    let mut stream = TcpStream::connect("127.0.0.1:8080").await?;
    println!("Connected to server");
    let mut io_reader = io::BufReader::new(io::stdin());

    loop {
        let mut stream_buffer = [0; 1024];

        // 0. 发送信息请求bash
        let msg = "bash ok";
        stream.write_all(msg.as_bytes()).await?;

        // 1. 获取cwd
        let len = stream.read(&mut stream_buffer).await?;
        if len == 0 {
            eprintln!("error reading answer from server");
            return Err(Error::new(ErrorKind::NotConnected, ""));
        }

        let recv_cwd = String::from_utf8_lossy(&stream_buffer);

        println!("{}", recv_cwd.replace('\0', ""));
        print!("$ ");
        io::stdout().flush().await?;

        // 2.1 读取输入
        let mut input = String::new();
        io_reader.read_line(&mut input).await?;
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input.trim().replace('\0', "") == "EXIT" {
            stream.write_all(input.as_bytes()).await?;
            return Ok(());
        }

        // 2.2 将指令发给server
        stream.write_all(input.as_bytes()).await?;

        // 2.3 读取返回信息，如果是需要继续输入信息的，则回复，否则不回复
        stream_buffer = [0; 1024];
        let n = stream.read(&mut stream_buffer).await?;
        if n == 0 {
            eprintln!("error reading answer from server");
            return Err(Error::new(ErrorKind::NotConnected, ""));
        }
        let msg = String::from_utf8_lossy(&stream_buffer);
        match msg.trim().replace('\0', "").as_str() {
            // 需要输入文件内容
            "INPUT FILE CONTENT" => {
                let inputs = read_file_content(&mut io_reader).await?;
                // 将得到的文件内容发送给server
                stream.write_all(inputs.as_bytes()).await?;
            }
            // 需要确认是否继续执行
            "CONFIRM COMMAND" => {
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
            "command ok." => continue,
            _ => {
                println!("{}", msg);
                continue;
            }
        };
        // 3. 等待server应答
        stream_buffer = [0; 1024];
        let n = stream.read(&mut stream_buffer).await?;
        if n == 0 {
            eprintln!("error reading answer from server");
            return Err(Error::new(ErrorKind::NotConnected, ""));
        }
        let msg = String::from_utf8_lossy(&stream_buffer);
        println!("{}", msg.trim().replace('\0', ""));
    }
}

async fn read_file_content(io_reader: &mut BufReader<Stdin>) -> io::Result<String> {
    let mut line = String::new();
    let mut inputs = String::new();
    while let Ok(bytes_read) = io_reader.read_line(&mut line).await {
        if bytes_read == 0 {
            break; // 读取完毕，输入结束
        }
        inputs.push_str(&line);
        line.clear();
    }
    Ok(inputs)
}
