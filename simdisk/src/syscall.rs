use std::fs;

use log::{error, info};

use crate::{
    block::sync_all_block_cache,
    dirent, file,
    inode::{FileMode, Inode},
    simple_fs::SFS,
};

/// 打印
#[allow(unused)]
pub fn info() {
    SFS.lock().info();
}

#[allow(unused)]
pub fn ls(detail:bool) {
    SFS.lock().current_inode.ls(detail);
}

#[allow(unused)]
pub fn ls_dir(path: &str,detail:bool) {
    temp_cd_and_do(path, false, |_| {
        ls(detail);
        true
    });
}

#[allow(unused)]
pub fn mkdir(name: &str) {
    temp_cd_and_do(name, true, |n| {
        if dirent::make_directory(n, &mut SFS.lock().current_inode).is_none() {
            info!("error in mkdir");
            false
        } else {
            true
        }
    });
}

#[allow(unused)]
pub fn rmdir(name: &str) {
    temp_cd_and_do(name, true, |n| {
        if dirent::remove_directory(n, &mut SFS.lock().current_inode).is_none() {
            info!("error in rmdir");
            false
        } else {
            true
        }
    });
}

#[allow(unused)]
pub fn cd(name: &str) {
    if dirent::cd(name).is_none() {
        info!("error in cd");
    }
}

#[allow(unused)]
pub fn new_file(name: &str, mode: FileMode) {
    temp_cd_and_do(name, true, |n| {
        if file::create_file(n, mode, &mut SFS.lock().current_inode, false, "").is_none() {
            info!("error in newfile");
            false
        } else {
            true
        }
    });
}

#[allow(unused)]
pub fn del(name: &str) {
    temp_cd_and_do(name, true, |n| {
        if file::remove_file(n, &mut SFS.lock().current_inode).is_none() {
            info!("error in del");
            false
        } else {
            true
        }
    });
}

#[allow(unused)]
pub fn cat(name: &str) {
    temp_cd_and_do(name, false, |n| {
        match file::open_file(n, &SFS.lock().current_inode) {
            Some(content) => {
                println!("{}", content);
                true
            }
            None => {
                info!("error in cat");
                false
            }
        }
    });
}

#[allow(unused)]
pub fn copy(source_path: &str, target_path: &str) {
    let mut content = String::new();
    // 访问host目录
    if source_path.starts_with("<host>") {
        let path = source_path.strip_prefix("<host>").unwrap();
        match fs::read_to_string(path) {
            Ok(string) => content = string,
            Err(err) => {
                error!("error reading host file");
                return;
            }
        }
    } else {
        temp_cd_and_do(source_path, false, |name| {
            match file::open_file(name, &SFS.lock().current_inode) {
                Some(source_content) => {
                    content = source_content;
                    true
                }
                None => {
                    info!("error in open source file");
                    false
                }
            }
        });
    }
    temp_cd_and_do(target_path, true, |name| {
        if file::create_file(
            name,
            FileMode::RDWR,
            &mut SFS.lock().current_inode,
            true,
            &content,
        )
        .is_none()
        {
            info!("error in creating target file");
            false
        } else {
            true
        }
    });
}

#[allow(unused)]
pub fn check() {
    SFS.lock().check();
}

/// 临时移动到指定目录,并执行f的操作，如果需要在操作之后更新块缓存，need_sync设置为true
fn temp_cd_and_do<F>(mut name: &str, need_sync: bool, f: F)
where
    F: FnOnce(&str) -> bool,
{
    let mut flag = false;
    let mut forward_wd = String::new();
    let mut forward_inode = Inode::default();
    if let Some((path, filename)) = name.rsplit_once('/') {
        // 记录先前的位置
        let fs = SFS.lock();
        (forward_wd, forward_inode) = (fs.cwd.clone(), fs.current_inode.clone());
        // 手动unlock fs防止死锁
        drop(fs);

        // 尝试进入目录
        if dirent::cd(path).is_none() {
            return;
        }
        flag = true;
        name = filename;
    }
    // 执行f的操作，成功返回true
    if f(name) {
        if flag {
            // 还原目录状态
            let mut fs = SFS.lock();
            fs.cwd = forward_wd;
            fs.current_inode = forward_inode;
        }
        if need_sync {
            sync_all_block_cache();
        }
    }
}
