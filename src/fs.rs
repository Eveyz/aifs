// src/fs.rs
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    Request, ReplyWrite,
};
use libc::{ENOENT, EACCES};
use std::ffi::OsStr;
use std::time::{Duration, UNIX_EPOCH};
use std::sync::{Arc, Mutex};
use crate::brain::searcher::Searcher;
// use std::collections::HashMap;

// 1. 根目录
// 2. ask 文件 (Write Only)
// 3. results 目录 (Read Only)
// 4+ 搜索结果文件 (Symlinks)

pub struct AiFS {
    searcher: Arc<Searcher>,
    // 存储搜索结果: index -> path
    current_results: Arc<Mutex<Vec<String>>>,
}

impl AiFS {
    pub fn new(searcher: Searcher) -> Self {
        Self {
            searcher: Arc::new(searcher),
            current_results: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

const TTL: Duration = Duration::from_secs(1);

impl Filesystem for AiFS {
    // 1. 属性获取
    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        let ts = UNIX_EPOCH;
        match ino {
            1 => reply.attr(&TTL, &FileAttr { ino: 1, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::Directory, perm: 0o755, nlink: 2, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 }),
            2 => reply.attr(&TTL, &FileAttr { ino: 2, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::RegularFile, perm: 0o666, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 }), // ask
            3 => reply.attr(&TTL, &FileAttr { ino: 3, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::Directory, perm: 0o755, nlink: 2, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 }), // results
            _ => {
                // Inode >= 4 是搜索结果
                let index = (ino - 4) as usize;
                let results = self.current_results.lock().unwrap();
                if index < results.len() {
                     // 这是一个软链接
                     reply.attr(&TTL, &FileAttr { ino: ino, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::Symlink, perm: 0o777, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 });
                } else {
                    reply.error(ENOENT);
                }
            }
        }
    }

    // 2. 查找文件名
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let ts = UNIX_EPOCH;
        if parent == 1 {
            match name.to_str() {
                Some("ask") => reply.entry(&TTL, &FileAttr { ino: 2, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::RegularFile, perm: 0o666, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 }, 0),
                Some("results") => reply.entry(&TTL, &FileAttr { ino: 3, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::Directory, perm: 0o755, nlink: 2, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 }, 0),
                _ => reply.error(ENOENT),
            }
        } else if parent == 3 {
            // 在 results 目录里找文件
             let name_str = name.to_str().unwrap();
             // 假设文件名是 "0", "1", "2"... 
             if let Ok(idx) = name_str.parse::<u64>() {
                 let results = self.current_results.lock().unwrap();
                 if (idx as usize) < results.len() {
                    let ino = idx + 4;
                    reply.entry(&TTL, &FileAttr { ino: ino, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::Symlink, perm: 0o777, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 }, 0);
                 } else {
                     reply.error(ENOENT);
                 }
             } else {
                 reply.error(ENOENT);
             }
        } else {
            reply.error(ENOENT);
        }
    }

    // 3. 读取目录
    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        if ino == 1 {
            if offset == 0 {
                let _ = reply.add(1, 0, FileType::Directory, ".");
                let _ = reply.add(1, 1, FileType::Directory, "..");
                let _ = reply.add(2, 2, FileType::RegularFile, "ask");
                let _ = reply.add(3, 3, FileType::Directory, "results");
            }
            reply.ok();
        } else if ino == 3 {
            // 列出搜索结果
            if offset == 0 {
                let _ = reply.add(3, 0, FileType::Directory, ".");
                let _ = reply.add(3, 1, FileType::Directory, "..");
                
                let results = self.current_results.lock().unwrap();
                for (i, _) in results.iter().enumerate() {
                    // 文件名直接叫 0, 1, 2, 3... 简单粗暴
                    let _ = reply.add(4 + i as u64, (i + 2) as i64, FileType::Symlink, &i.to_string());
                }
            }
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }

    // 4. 写入 (核心：拦截 echo "query" > ask)
    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, _offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyWrite) {
        if ino == 2 {
            let query = String::from_utf8_lossy(data).to_string();
            println!(">>> 收到 FUSE 指令: {}", query);
            
            // 触发搜索
            let searcher = self.searcher.clone();
            // 注意：这里需要 Searcher 实现一个返回 path 的方法
            // 为了简单，我们手动 copy searcher 的逻辑片段
            // 实际工程中应该封装好
            let query_clone = query.clone();
            let results_store = self.current_results.clone();
            
            // 阻塞执行搜索 (FUSE write 是同步的)
            // 你需要在 searcher.rs 里加一个 search_return_paths 方法
            let paths = futures::executor::block_on(async {
                 let vec = searcher.get_client().get_embedding(&query_clone).await.unwrap_or(vec![]);
                 searcher.get_store().search("my_files", vec, 10).await.unwrap_or(vec![])
            });

            let mut lock = results_store.lock().unwrap();
            *lock = paths;
            println!(">>> 搜索完成，更新了 {} 个结果", lock.len());

            reply.written(data.len() as u32);
        } else {
            reply.error(EACCES);
        }
    }

    // 5. 读取软链接 (让 ls -l 显示真实路径)
    fn readlink(&mut self, _req: &Request, ino: u64, reply: ReplyData) {
        if ino >= 4 {
             let index = (ino - 4) as usize;
             let results = self.current_results.lock().unwrap();
             if let Some(path) = results.get(index) {
                 reply.data(path.as_bytes());
             } else {
                 reply.error(ENOENT);
             }
        } else {
            reply.error(ENOENT);
        }
    }

    // --- 新增这个方法 ---
    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>, // Shell 用 > 时会传入 size=0
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        // 我们只允许对 "ask" 文件 (inode=2) 做操作
        if ino == 2 {
            // 无论 Shell想改成什么样，我们都返回一个标准的属性
            // 只要返回 reply.attr()，Shell 就认为操作成功了
            let ts = UNIX_EPOCH;
            reply.attr(
                &TTL,
                &FileAttr {
                    ino: 2,
                    size: 0, // 假装变成 0 了
                    blocks: 0,
                    atime: ts,
                    mtime: ts,
                    ctime: ts,
                    crtime: ts,
                    kind: FileType::RegularFile,
                    perm: 0o666,
                    nlink: 1,
                    uid: 1000,
                    gid: 1000,
                    rdev: 0,
                    flags: 0,
                    blksize: 512,
                },
            );
        } else {
            reply.error(ENOENT);
        }
    }
}