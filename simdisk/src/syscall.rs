use std::{future::Future, io::Error, pin::Pin, sync::Arc};

use tokio::net::TcpStream;

use crate::{
    block::sync_all_block_cache,
    dirent, file,
    inode::FileMode,
    simple_fs::{self, SFS},
};

/// 打印
pub async fn info(cwd: &str) -> Result<Option<String>, Error> {
    let res = temp_cd_and_do(&[cwd, "/"].concat(), false, |_| {
        Box::pin(async {
            let fs = Arc::clone(&SFS);
            let read_lock = fs.read().await;
            Ok(Some(read_lock.info().await))
        })
    })
    .await?;
    trace!("finished cmd: info");
    Ok(res)
}

/// 展示目录信息
pub async fn ls(username: &str, path: &str, detail: bool) -> Result<Option<String>, Error> {
    let infos = temp_cd_and_do(&[path, "/"].concat(), false, |_| {
        Box::pin(async {
            let fs = Arc::clone(&SFS);
            let read_lock = fs.read().await;
            Ok(Some(read_lock.current_inode.ls(username, detail).await))
        })
    })
    .await?;
    trace!("finished cmd: ls_dir");
    Ok(infos)
}

/// 创建目录
pub async fn mkdir(username: &str, dir_name: &str) -> Result<(), Error> {
    temp_cd_and_do(dir_name, true, |n| {
        Box::pin(async move {
            let (gid, uid) = get_current_user_ids(username).await;
            let fs = Arc::clone(&SFS);
            let mut fs_write_lock = fs.write().await;
            dirent::make_directory(n, &mut fs_write_lock.current_inode, gid, uid).await
        })
    })
    .await?;
    trace!("finished cmd: mkdir");
    Ok(())
}

/// 删除目录，包括其中的文件和子目录
pub async fn rmdir(username: &str, dir_name: &str, socket: &mut TcpStream) -> Result<(), Error> {
    temp_cd_and_do(dir_name, true, |n| {
        Box::pin(async move {
            let (gid, _) = get_current_user_ids(username).await;
            let fs = Arc::clone(&SFS);
            let mut w = fs.write().await;
            dirent::remove_directory(n, &mut w.current_inode, socket, gid).await
        })
    })
    .await?;
    trace!("finished cmd: rmdir");
    Ok(())
}

/// 移动路径
pub async fn cd(path: &str) -> Result<(), Error> {
    // 目录不存在会抛出err
    dirent::cd(path).await?;
    // 还原状态
    let fs = Arc::clone(&SFS);
    let mut fs_write_lock = fs.write().await;
    fs_write_lock.current_inode = fs_write_lock.root_inode.clone();
    trace!("finished cmd: cd");
    Ok(())
}

/// 创建新文件
pub async fn new_file(
    username: &str,
    filename: &str,
    mode: FileMode,
    socket: &mut TcpStream,
) -> Result<(), Error> {
    temp_cd_and_do(filename, true, |n| {
        Box::pin(async move {
            let user_id = get_current_user_ids(username).await;
            let fs = Arc::clone(&SFS);
            let mut fs_write_lock = fs.write().await;
            file::create_file(
                n,
                mode,
                &mut fs_write_lock.current_inode,
                false,
                "",
                socket,
                user_id,
            )
            .await
        })
    })
    .await?;
    trace!("finished cmd: newfile");
    Ok(())
}

/// 删除文件
pub async fn del(username: &str, filename: &str) -> Result<(), Error> {
    temp_cd_and_do(filename, true, |n| {
        Box::pin(async move {
            let (gid, _) = get_current_user_ids(username).await;
            let fs = Arc::clone(&SFS);
            let mut w = fs.write().await;
            file::remove_file(n, &mut w.current_inode, gid).await
        })
    })
    .await?;
    trace!("finished cmd: del [{}]", filename);
    Ok(())
}

/// 获取文件内容
pub async fn cat(name: &str) -> Result<Option<String>, Error> {
    let content = temp_cd_and_do(name, false, |n| {
        Box::pin(async move {
            let fs = Arc::clone(&SFS);
            let read_lock = fs.read().await;
            file::get_file_content(n, &read_lock.current_inode).await
        })
    })
    .await?;
    trace!("finished cmd: cat [{}]", name);
    Ok(Some(content))
}

/// 复制文件
pub async fn copy(
    username: &str,
    source_path: &str,
    target_path: &str,
    socket: &mut TcpStream,
) -> Result<(), Error> {
    let content = if source_path.starts_with("<host>") {
        // 访问host目录
        let path = source_path.strip_prefix("<host>").unwrap();
        std::fs::read_to_string(path)?
    } else {
        // 从系统中取出内容
        temp_cd_and_do(source_path, false, |name| {
            Box::pin(async {
                let fs = Arc::clone(&SFS);
                let read_lock = fs.read().await;
                file::get_file_content(name, &read_lock.current_inode).await
            })
        })
        .await?
    };
    trace!("finished get source contents");
    temp_cd_and_do(target_path, true, |name| {
        Box::pin(async move {
            let user_id = get_current_user_ids(username).await;
            let fs = Arc::clone(&SFS);
            let mut w = fs.write().await;
            file::create_file(
                name,
                FileMode::RDWR,
                &mut w.current_inode,
                true,
                &content,
                socket,
                user_id,
            )
            .await
        })
    })
    .await?;
    trace!("finished cmd: copy [{}] to [{}]", source_path, target_path);
    Ok(())
}

/// 查看超级块是否损坏，并查看位图是否出错
pub async fn check() -> Result<(), Error> {
    let fs = Arc::clone(&SFS);
    fs.write().await.reset_sp().await;
    simple_fs::check_bitmaps_and_fix().await?;
    trace!("finished cmd: check");
    Ok(())
}

/// 获取所有用户信息
pub async fn get_users_info(username: &str) -> Result<Option<String>, Error> {
    let fs = Arc::clone(&SFS);
    let read_lock = fs.read().await;
    let current_gid = read_lock.get_user_gid(username)?;
    let users = read_lock.get_users_info(current_gid)?;
    trace!("finished cmd: users");
    Ok(Some(format!("{:#?}", users)))
}

/// 格式化
pub async fn formatting() -> Result<(), Error> {
    let fs = Arc::clone(&SFS);
    fs.write().await.force_clear().await;
    trace!("finished cmd: formatting");
    Ok(())
}

/// 临时移动到指定目录,并执行f的操作，
/// 如果需要在操作之后更新块缓存，need_sync设置为true
///
/// 在尝试寻找路径的时候如果找不到返回Err
///
/// f 返回 Error(msg)代表f执行失败，返回ok代表成功
///
/// 最后该函数返回从f得到的失败信息err结果，f成功则返回ok
async fn temp_cd_and_do<'a, F, T>(mut name: &'a str, need_sync: bool, f: F) -> Result<T, Error>
where
    F: FnOnce(&'a str) -> Pin<Box<dyn Future<Output = Result<T, Error>> + 'a + Send>>,
{
    let mut flag = false;
    if let Some((path, filename)) = name.rsplit_once('/') {
        // 尝试进入目录
        dirent::cd(path).await?;
        flag = true;
        name = filename;
    }
    // 执行f的操作，失败则f的错误信息
    let res = match f(name).await {
        Ok(ok) => {
            if need_sync {
                sync_all_block_cache().await?;
            }
            Ok(ok)
        }
        Err(err) => Err(err),
    };
    if flag {
        // 还原目录状态
        let fs = Arc::clone(&SFS);
        let mut w = fs.write().await;
        w.current_inode = w.root_inode.clone();
    }
    res
}

/// 获取当前用户的id
async fn get_current_user_ids(username: &str) -> (u16, u16) {
    let fs = Arc::clone(&SFS);
    let r = fs.read().await;
    let ids = r.get_user_ids(username).unwrap();
    (ids.gid, ids.uid)
}
