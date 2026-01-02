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
use std::collections::HashSet;
use tokio::runtime::Handle;
use std::cmp::min;

pub struct AiFS {
    searcher: Arc<Searcher>,
    current_results: Arc<Mutex<Vec<String>>>,
    current_answer: Arc<Mutex<String>>,
}

impl AiFS {
    pub fn new(searcher: Searcher) -> Self {
        Self {
            searcher: Arc::new(searcher),
            current_results: Arc::new(Mutex::new(Vec::new())),
            current_answer: Arc::new(Mutex::new("请先在 ask 文件中输入问题，然后查看此处...".to_string())),
        }
    }
}

const TTL: Duration = Duration::from_secs(1);
const NO_CACHE: Duration = Duration::from_secs(0);

impl Filesystem for AiFS {
    // 1. 获取属性
    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        let ts = UNIX_EPOCH;
        match ino {
            // Root
            1 => reply.attr(&TTL, &FileAttr { 
                ino: 1, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, 
                kind: FileType::Directory, perm: 0o755, nlink: 2, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 
            }),
            // ask
            2 => reply.attr(&TTL, &FileAttr { 
                ino: 2, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, 
                kind: FileType::RegularFile, perm: 0o666, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 
            }),
            // results dir
            3 => reply.attr(&NO_CACHE, &FileAttr { 
                ino: 3, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, 
                kind: FileType::Directory, perm: 0o755, nlink: 2, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 
            }),
            // answer.txt
            4 => {
                let answer = self.current_answer.lock().unwrap();
                reply.attr(&NO_CACHE, &FileAttr { 
                    ino: 4, 
                    size: answer.len() as u64, 
                    blocks: 1, 
                    atime: ts, mtime: ts, ctime: ts, crtime: ts, 
                    kind: FileType::RegularFile, 
                    perm: 0o444, 
                    nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 
                })
            },
            // Search results (symlinks)
            _ => {
                if ino >= 5 {
                    let index = (ino - 5) as usize;
                    let results = self.current_results.lock().unwrap();
                    if index < results.len() {
                        reply.attr(&NO_CACHE, &FileAttr { 
                            ino: ino, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, 
                            kind: FileType::Symlink, perm: 0o777, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 
                        });
                    } else {
                        reply.error(ENOENT);
                    }
                } else {
                    reply.error(ENOENT);
                }
            }
        }
    }

    fn setattr(
        &mut self, _req: &Request, ino: u64, _mode: Option<u32>, _uid: Option<u32>, _gid: Option<u32>, _size: Option<u64>, 
        _atime: Option<fuser::TimeOrNow>, _mtime: Option<fuser::TimeOrNow>, _ctime: Option<std::time::SystemTime>, 
        _fh: Option<u64>, _crtime: Option<std::time::SystemTime>, _chgtime: Option<std::time::SystemTime>, 
        _bkuptime: Option<std::time::SystemTime>, _flags: Option<u32>, reply: ReplyAttr,
    ) {
        if ino == 2 {
            let ts = UNIX_EPOCH;
            reply.attr(&TTL, &FileAttr { 
                ino: 2, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, 
                kind: FileType::RegularFile, perm: 0o666, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 
            });
        } else {
            reply.error(ENOENT);
        }
    }

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let ts = UNIX_EPOCH;
        if parent == 1 {
            match name.to_str() {
                Some("ask") => reply.entry(&TTL, &FileAttr { 
                    ino: 2, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, 
                    kind: FileType::RegularFile, perm: 0o666, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 
                }, 0),
                Some("results") => reply.entry(&NO_CACHE, &FileAttr { 
                    ino: 3, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, 
                    kind: FileType::Directory, perm: 0o755, nlink: 2, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 
                }, 0),
                Some("answer.txt") => {
                    let answer = self.current_answer.lock().unwrap();
                    reply.entry(&NO_CACHE, &FileAttr { 
                        ino: 4, size: answer.len() as u64, blocks: 1, atime: ts, mtime: ts, ctime: ts, crtime: ts, 
                        kind: FileType::RegularFile, perm: 0o444, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 
                    }, 0)
                },
                _ => reply.error(ENOENT),
            }
        } else if parent == 3 {
             let name_str = name.to_str().unwrap();
             if let Ok(idx) = name_str.parse::<u64>() {
                 let results = self.current_results.lock().unwrap();
                 if (idx as usize) < results.len() {
                    let ino = idx + 5;
                    reply.entry(&NO_CACHE, &FileAttr { 
                        ino: ino, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, 
                        kind: FileType::Symlink, perm: 0o777, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 
                    }, 0);
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

    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        if ino == 1 {
            if offset == 0 {
                let _ = reply.add(1, 0, FileType::Directory, ".");
                let _ = reply.add(1, 1, FileType::Directory, "..");
                let _ = reply.add(2, 2, FileType::RegularFile, "ask");
                let _ = reply.add(3, 3, FileType::Directory, "results");
                let _ = reply.add(4, 4, FileType::RegularFile, "answer.txt");
            }
            reply.ok();
        } else if ino == 3 {
            if offset == 0 {
                let _ = reply.add(3, 0, FileType::Directory, ".");
                let _ = reply.add(3, 1, FileType::Directory, "..");
                let results = self.current_results.lock().unwrap();
                for (i, _) in results.iter().enumerate() {
                    let _ = reply.add(5 + i as u64, (i + 2) as i64, FileType::Symlink, &i.to_string());
                }
            }
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }

    fn read(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyData) {
        if ino == 4 {
            let answer = self.current_answer.lock().unwrap();
            let bytes = answer.as_bytes();
            if offset >= bytes.len() as i64 {
                reply.data(&[]);
            } else {
                let start = offset as usize;
                let end = min(bytes.len(), start + size as usize);
                reply.data(&bytes[start..end]);
            }
        } else {
            reply.error(ENOENT);
        }
    }

    fn readlink(&mut self, _req: &Request, ino: u64, reply: ReplyData) {
        if ino >= 5 {
             let index = (ino - 5) as usize;
             let results = self.current_results.lock().unwrap();
             if let Some(path) = results.get(index) {
                 let linux_path = path.replace("\\", "/");
                 reply.data(linux_path.as_bytes());
             } else {
                 reply.error(ENOENT);
             }
        } else {
            reply.error(ENOENT);
        }
    }

    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, _offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyWrite) {
        if ino == 2 {
            let raw_str = String::from_utf8_lossy(data);
            let query = raw_str.trim().to_string();
            
            if query.is_empty() {
                reply.written(data.len() as u32);
                return;
            }

            println!(">>> [FUSE Write] 收到问题: [{}]", query);
            
            let searcher = self.searcher.clone();
            let results_store = self.current_results.clone();
            let answer_store = self.current_answer.clone();
            let query_clone = query.clone();

            let handle = Handle::current();

            let (paths, answer_text) = handle.block_on(async {
                 println!("   -> 1. 正在搜索文件...");
                 let embedding_result = searcher.get_client().get_embedding(&query_clone).await;
                 
                 let found_paths = match embedding_result {
                     Ok(vec) => searcher.get_store().search("my_files", vec, 20).await.unwrap_or_default(),
                     Err(_) => vec![]
                 };

                 println!("   -> 2. 正在生成回答 (这可能需要几秒钟)...");
                 let rag_answer = match searcher.ask_question(&query_clone).await {
                     Ok(ans) => ans,
                     Err(e) => format!("生成失败: {}", e),
                 };
                 
                 (found_paths, rag_answer)
            });

            let mut unique_paths = Vec::new();
            let mut seen = HashSet::new();
            for p in paths {
                let normalized = p.replace("\\", "/"); 
                if !seen.contains(&normalized) {
                    seen.insert(normalized.clone());
                    unique_paths.push(normalized);
                }
            }
            *results_store.lock().unwrap() = unique_paths;
            *answer_store.lock().unwrap() = answer_text;
            
            println!(">>> [FUSE Write] 完成！请查看 answer.txt");
            reply.written(data.len() as u32);
        } else {
            reply.error(EACCES);
        }
    }
}