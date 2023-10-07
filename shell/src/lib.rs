//! socket公用的常量标记
use std::time::Duration;
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::sleep,
};

pub const SOCKET_ADDR: &str = "127.0.0.1:8080";
pub const CONTENT_SOCKET_ADDR: &str = "127.0.0.1:8081";
pub const BASH_REQUEST: &str = "BASH OK";
pub const EMPTY_INPUT: &str = "EMPTY INPUT";
pub const EXIT_MSG: &str = "EXIT";
pub const INPUT_FILE_CONTENT: &str = "INPUT FILE CONTENT";
pub const COMMAND_CONFIRM: &str = "COMMAND CONFIRM";
pub const COMMAND_FINISHED: &str = "COMMAND OK";
pub const LOGIN_SUCCESS: &str = "LOGIN_SUCCESS";
pub const REGIST_SUCCESS: &str = "REGIST SUCCESS";
pub const RECEIVE_CONTENTS: &str = "RECEIVE CONTENTS";
pub const READY_RECEIVE_CONTENTS: &str = "READY!";
pub const HELP_REQUEST:&str = "HELP";
pub const ERROR_MESSAGE_PREFIX:&str = "ErrMsg:";
pub const SOCKET_BUFFER_SIZE: usize = 128;

/// 通过8081发送长内容，送达后关闭socket
pub async fn send_content(content: String) -> io::Result<()> {
    let mut stream;
    let mut retry = 0;
    loop {
        // 轮询等待tcp打开
        match TcpStream::connect(CONTENT_SOCKET_ADDR).await {
            Ok(s) => {
                stream = s;
                break;
            }
            Err(e) => {
                retry += 1;
                if retry > 10 {
                    return Err(e);
                }
                sleep(Duration::from_millis(50)).await;
            }
        }
    }
    stream.write_all(content.as_bytes()).await?;
    stream.shutdown().await
}

/// 开始临时监听8081，接受长内容，完成后关闭socket
pub async fn receive_content() -> io::Result<String> {
    let (mut socket, _) = TcpListener::bind(CONTENT_SOCKET_ADDR)
        .await?
        .accept()
        .await?;
    // 通知发送方tcp已打开
    let mut buffer = String::new();
    let n = socket.read_to_string(&mut buffer).await?;
    socket.shutdown().await?;
    if n == 0 {
        Err(std::io::Error::new(
            io::ErrorKind::InvalidData,
            "read 0 byte",
        ))
    } else {
        Ok(buffer)
    }
}
