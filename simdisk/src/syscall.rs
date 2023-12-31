use std::{future::Future, io, pin::Pin, sync::Arc};

use tokio::net::TcpStream;

use crate::{
    block::{self, sync_all_block_cache, BLOCK_CACHE_MANAGER},
    dirent, file,
    fs_constants::SYNC_BLOCK_DURATION,
    inode::{FileMode, Inode},
    simple_fs::{self, SFS},
    user::{able_to_modify, UserIdType},
};

/// 打印
pub async fn info() -> io::Result<Option<String>> {
    let fs = Arc::clone(&SFS);
    let read_lock = fs.read().await;
    let res = Some(read_lock.info().await);
    trace!("finished cmd: info");
    Ok(res)
}

/// 展示目录信息
pub async fn ls(username: &str, path: &str, detail: bool) -> io::Result<Option<String>> {
    let absolute_path = [path, "/"].concat();
    let infos = temp_cd_and_do(&absolute_path, false, |_, current_inode| {
        Box::pin(async move { Ok(Some(current_inode.ls(username, detail).await)) })
    })
    .await?;
    trace!("finished cmd: ls_dir");
    Ok(infos)
}

/// 创建目录
pub async fn mkdir(username: &str, dir_name_absolute: &str) -> io::Result<()> {
    temp_cd_and_do(dir_name_absolute, true, |name, mut current_inode| {
        Box::pin(async move {
            let (gid, uid) = get_current_user_ids(username).await;
            dirent::make_directory(name, &mut current_inode, gid, uid).await
        })
    })
    .await?;
    trace!("finished cmd: mkdir");
    Ok(())
}

/// 删除目录，包括其中的文件和子目录
pub async fn rmdir(
    username: &str,
    dir_name_absolute: &str,
    socket: &mut TcpStream,
) -> io::Result<()> {
    temp_cd_and_do(dir_name_absolute, true, |name, mut current_inode| {
        Box::pin(async move {
            let gid = get_current_user_gid(username).await;
            dirent::remove_directory(name, &mut current_inode, socket, gid).await
        })
    })
    .await?;
    trace!("finished cmd: rmdir");
    Ok(())
}

/// 移动路径
pub async fn cd(absolute_path: &str) -> io::Result<()> {
    // 目录不存在会抛出err
    let root = Arc::clone(&SFS).read().await.root_inode.clone();
    dirent::cd(absolute_path, &root).await?;
    trace!("finished cmd: cd");
    Ok(())
}

/// 创建新文件
pub async fn new_file(
    username: &str,
    filename_absolute: &str,
    mode: FileMode,
    socket: &mut TcpStream,
) -> io::Result<()> {
    temp_cd_and_do(filename_absolute, true, |filename, mut current_inode| {
        Box::pin(async move {
            let user_id = get_current_user_ids(username).await;
            file::create_file(
                filename,
                mode,
                &mut current_inode,
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
pub async fn del(username: &str, filename_absolute: &str) -> io::Result<()> {
    temp_cd_and_do(filename_absolute, true, |filename, mut current_inode| {
        Box::pin(async move {
            let gid = get_current_user_gid(username).await;
            file::remove_file(filename, &mut current_inode, gid).await
        })
    })
    .await?;
    trace!("finished cmd: del [{}]", filename_absolute);
    Ok(())
}

/// 获取文件内容
pub async fn cat(filename_absolute: &str) -> io::Result<Option<String>> {
    let content = temp_cd_and_do(filename_absolute, false, |filename, current_inode| {
        Box::pin(async move { file::get_file_content(filename, &current_inode).await })
    })
    .await?;
    trace!("finished cmd: cat [{}]", filename_absolute);
    Ok(Some(content))
}

/// 复制文件
pub async fn copy(
    username: &str,
    source_path: &str,
    target_path: &str,
    socket: &mut TcpStream,
) -> io::Result<()> {
    let content = if source_path.starts_with("<host>") {
        // 访问host目录
        let path = source_path.strip_prefix("<host>").unwrap();
        std::fs::read_to_string(path)?
    } else {
        // 从系统中取出内容
        temp_cd_and_do(source_path, false, |name, current_inode| {
            Box::pin(async move { file::get_file_content(name, &current_inode).await })
        })
        .await?
    };
    trace!("finished get source contents");
    temp_cd_and_do(target_path, true, |name, mut current_inode| {
        Box::pin(async move {
            let user_id = get_current_user_ids(username).await;
            file::create_file(
                name,
                FileMode::RDWR,
                &mut current_inode,
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
pub async fn check() -> io::Result<()> {
    simple_fs::check_bitmaps_and_fix().await?;
    trace!("finished cmd: check");
    Ok(())
}

/// 获取所有用户信息
pub async fn get_users_info(username: &str) -> io::Result<Option<String>> {
    let fs = Arc::clone(&SFS);
    let read_lock = fs.read().await;
    let current_gid = read_lock.get_user_gid(username)?;
    let users = read_lock.get_users_info(current_gid)?;
    trace!("finished cmd: users");
    Ok(Some(format!("{:#?}", users)))
}

/// 格式化
pub async fn formatting(username: &str) -> io::Result<()> {
    let gid = get_current_user_gid(username).await;
    if !able_to_modify(gid, 0) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "not in root",
        ));
    }
    let fs = Arc::clone(&SFS);
    fs.write().await.force_clear().await;
    trace!("finished cmd: formatting");
    Ok(())
}

pub async fn set_block_cache_method(method: &str) -> io::Result<()> {
    let manager = Arc::clone(&BLOCK_CACHE_MANAGER);
    let mut write_lock = manager.write().await;
    match method.to_lowercase().as_str() {
        "instant" => write_lock.cahce_method = block::CacheMethod::Immediately,
        "exit" => write_lock.cahce_method = block::CacheMethod::OnExit,
        "tick" => {
            write_lock.cahce_method = block::CacheMethod::Scheduled;
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(SYNC_BLOCK_DURATION)).await;
                if !block::is_sync_scheduled().await {
                    return;
                }
                if let Err(e) = sync_all_block_cache().await {
                    error!("{}", e);
                }
            });
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "no such mode")),
    }
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
async fn temp_cd_and_do<'a, F, T>(absolute_path: &'a str, need_sync: bool, f: F) -> io::Result<T>
where
    F: FnOnce(&'a str, Inode) -> Pin<Box<dyn Future<Output = io::Result<T>> + 'a + Send>>,
{
    let mut current_inode = Arc::clone(&SFS).read().await.root_inode.clone();
    let mut name = None;
    if let Some((path, filename)) = absolute_path.rsplit_once('/') {
        // 尝试进入目录
        current_inode = dirent::cd(path, &current_inode).await?;
        name = Some(filename)
    }
    // 执行f的操作，失败则f的错误信息
    match f(name.unwrap(), current_inode).await {
        Ok(ok) => {
            if need_sync && block::is_sync_immediately().await {
                sync_all_block_cache().await?;
            }
            Ok(ok)
        }
        Err(err) => Err(err),
    }
}

/// 获取当前用户的id
async fn get_current_user_ids(username: &str) -> (UserIdType, UserIdType) {
    let fs = Arc::clone(&SFS);
    let r = fs.read().await;
    let ids = r.get_user_ids(username).unwrap();
    (ids.gid, ids.uid)
}

/// 获取当前用户的gid
async fn get_current_user_gid(username: &str) -> UserIdType {
    let fs = Arc::clone(&SFS);
    let r = fs.read().await;
    r.get_user_gid(username).unwrap()
}
