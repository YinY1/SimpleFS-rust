use std::{fs, future::Future, io::Error, pin::Pin, sync::Arc};

use tokio::net::TcpStream;

use crate::{
    block::sync_all_block_cache,
    dirent, file,
    inode::{FileMode, Inode},
    simple_fs::SFS,
};

/// 打印
#[allow(unused)]
pub async fn info() -> Result<Option<String>, Error> {
    let fs = Arc::clone(&SFS);
    let res = fs.read().await.info().await;
    trace!("finished cmd: info");
    Ok(Some(res))
}

#[allow(unused)]
pub async fn ls(detail: bool) -> Result<Option<String>, Error> {
    let fs = Arc::clone(&SFS);
    let infos = fs.read().await.current_inode.ls(detail).await;
    trace!("finished cmd: ls");
    Ok(Some(infos))
}

#[allow(unused)]
pub async fn ls_dir(path: &str, detail: bool) -> Result<Option<String>, Error> {
    let mut infos = None;
    temp_cd_and_do(path, false, |_| {
        Box::pin(async {
            infos = ls(detail).await.unwrap();
            Ok(())
        })
    })
    .await?;
    trace!("finished cmd: ls_dir");
    Ok(infos)
}

#[allow(unused)]
pub async fn mkdir(name: &str) -> Result<(), Error> {
    temp_cd_and_do(name, true, |n| {
        Box::pin(async move {
            let mut fs = Arc::clone(&SFS);
            let mut w = fs.write().await;
            dirent::make_directory(n, &mut w.current_inode).await
        })
    })
    .await?;
    trace!("finished cmd: mkdir");
    Ok(())
}

#[allow(unused)]
pub async fn rmdir(name: &str, socket: &mut TcpStream) -> Result<(), Error> {
    temp_cd_and_do(name, true, |n| {
        Box::pin(async move {
            let fs = Arc::clone(&SFS);
            let mut w = fs.write().await;
            dirent::remove_directory(n, &mut w.current_inode, socket).await
        })
    })
    .await?;
    trace!("finished cmd: rmdir");
    Ok(())
}

#[allow(unused)]
pub async fn cd(name: &str) -> Result<(), Error> {
    dirent::cd(name).await?;
    trace!("finished cmd: cd");
    Ok(())
}

#[allow(unused)]
pub async fn new_file(name: &str, mode: FileMode, socket: &mut TcpStream) -> Result<(), Error> {
    temp_cd_and_do(name, true, |n| {
        Box::pin(async move {
            let fs = Arc::clone(&SFS);
            let mut w = fs.write().await;
            file::create_file(n, mode, &mut w.current_inode, false, "", socket).await
        })
    })
    .await?;
    trace!("finished cmd: newfile");
    Ok(())
}

#[allow(unused)]
pub async fn del(name: &str) -> Result<(), Error> {
    temp_cd_and_do(name, true, |n| {
        Box::pin(async move {
            let fs = Arc::clone(&SFS);
            let mut w = fs.write().await;
            file::remove_file(n, &mut w.current_inode).await
        })
    })
    .await?;
    trace!("finished cmd: del [{}]", name);
    Ok(())
}

#[allow(unused)]
pub async fn cat(name: &str) -> Result<Option<String>, Error> {
    let content = temp_cd_and_do(name, false, |n| {
        Box::pin(async move {
            let fs = Arc::clone(&SFS);
            let r = fs.read().await;
            file::open_file(n, &r.current_inode).await
        })
    })
    .await?;
    trace!("finished cmd: cat [{}]", name);
    Ok(Some(content))
}

#[allow(unused)]
pub async fn copy(
    source_path: &str,
    target_path: &str,
    socket: &mut TcpStream,
) -> Result<(), Error> {
    let mut content = String::new();
    // 访问host目录
    if source_path.starts_with("<host>") {
        let path = source_path.strip_prefix("<host>").unwrap();
        content = fs::read_to_string(path)?;
    } else {
        temp_cd_and_do(source_path, false, |name| {
            Box::pin(async {
                let fs = Arc::clone(&SFS);
                let r = fs.read().await;
                content = file::open_file(name, &r.current_inode).await?;
                Ok(())
            }) as _
        })
        .await;
    }
    trace!("finished get source contents");
    temp_cd_and_do(target_path, true, |name| {
        Box::pin(async move {
            let fs = Arc::clone(&SFS);
            let mut w = fs.write().await;
            file::create_file(
                name,
                FileMode::RDWR,
                &mut w.current_inode,
                true,
                &content,
                socket,
            )
            .await
        })
    })
    .await?;
    trace!("finished cmd: copy [{}] to [{}]", source_path, target_path);
    Ok(())
}

#[allow(unused)]
pub async fn check() -> Result<(), Error> {
    SFS.write().await.check().await;
    trace!("finished cmd: check");
    Ok(())
}

/// 临时移动到指定目录,并执行f的操作，
/// 如果需要在操作之后更新块缓存，need_sync设置为true
///
/// 在尝试寻找路径的时候如果找不到返回一条错误信息String
///
/// f 返回 Error(msg)代表f执行失败，返回ok代表成功
///
/// 最后该函数返回从f得到的失败信息err结果，f成功则返回ok
async fn temp_cd_and_do<'a, F, T>(mut name: &'a str, need_sync: bool, f: F) -> Result<T, Error>
where
    F: FnOnce(&'a str) -> Pin<Box<dyn Future<Output = Result<T, Error>> + 'a + Send>>,
{
    let mut flag = false;
    let mut forward_wd = String::new();
    let mut forward_inode = Inode::default();
    if let Some((path, filename)) = name.rsplit_once('/') {
        // 记录先前的位置
        let fs = Arc::clone(&SFS);
        let r = fs.read().await;
        (forward_wd, forward_inode) = (r.cwd.clone(), r.current_inode.clone());
        // 手动unlock fs防止死锁
        drop(r);

        // 尝试进入目录
        dirent::cd(path).await?;
        flag = true;
        name = filename;
    }
    // 执行f的操作，失败则f的错误信息
    match f(name).await {
        Ok(ok) => {
            if flag {
                // 还原目录状态
                let fs = Arc::clone(&SFS);
                let mut w = fs.write().await;
                w.cwd = forward_wd;
                w.current_inode = forward_inode;
            }
            if need_sync {
                sync_all_block_cache().await?;
            }
            Ok(ok)
        }
        Err(err) => Err(err),
    }
}
