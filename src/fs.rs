// src/fs.rs
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    Request, ReplyWrite,
};
use libc::{ENOENT, EACCES};
use std::ffi::OsStr;
use std::time::{Duration, UNIX_EPOCH, SystemTime};
use std::sync::{Arc, Mutex};
use crate::brain::searcher::Searcher;
use std::collections::{HashSet, HashMap};
use tokio::runtime::Handle;
use std::cmp::min;
use std::path::Path;

pub struct AiFS {
    searcher: Arc<Searcher>,
    
    // --- 搜索功能区 ---
    current_results: Arc<Mutex<Vec<String>>>, // 搜索结果 (Inode 5+)
    current_answer: Arc<Mutex<String>>,       // RAG 回答 (Inode 4)

    // --- 语义视图功能区 (新增) ---
    // 缓存: Tag名字 -> 文件路径列表
    tag_cache: Arc<Mutex<HashMap<String, Vec<String>>>>,
    // 缓存: 所有已知文件的去重列表 (用于生成稳定的 Inode)
    // 索引 i 对应 Inode 100000 + i
    global_file_list: Arc<Mutex<Vec<String>>>,
    // 上次刷新时间
    last_tag_refresh: Arc<Mutex<SystemTime>>,
}

impl AiFS {
    pub fn new(searcher: Searcher) -> Self {
        Self {
            searcher: Arc::new(searcher),
            current_results: Arc::new(Mutex::new(Vec::new())),
            current_answer: Arc::new(Mutex::new("请先在 ask 文件中输入问题...".to_string())),
            
            tag_cache: Arc::new(Mutex::new(HashMap::new())),
            global_file_list: Arc::new(Mutex::new(Vec::new())),
            last_tag_refresh: Arc::new(Mutex::new(UNIX_EPOCH)),
        }
    }

    // 核心辅助函数：从数据库刷新 Tags 视图
    fn refresh_tags_if_needed(&self) {
        let mut last = self.last_tag_refresh.lock().unwrap();
        // 10秒缓存，防止频繁查库
        if last.elapsed().unwrap_or(Duration::from_secs(100)) < Duration::from_secs(10) {
            return;
        }

        println!(">>> [FUSE] 正在刷新 Tags 视图...");
        let store = self.searcher.get_store(); 
        let handle = Handle::current();
        
        // 调用 store.get_all_files_with_tags (需要在 store.rs 里实现)
        let files = handle.block_on(async {
            store.get_all_files_with_tags("my_files").await.unwrap_or_default()
        });

        let mut new_tag_map: HashMap<String, Vec<String>> = HashMap::new();
        let mut unique_files: HashSet<String> = HashSet::new();

        for (path, tags_str) in files {
            // 路径标准化
            let clean_path = path.replace("\\", "/");
            unique_files.insert(clean_path.clone());

            // tags_str 可能是 "networking, utils"
            for tag in tags_str.split(',') {
                let clean_tag = tag.trim().to_lowercase(); // 统一小写
                if !clean_tag.is_empty() {
                    new_tag_map.entry(clean_tag).or_default().push(clean_path.clone());
                }
            }
        }

        // 更新缓存
        *self.tag_cache.lock().unwrap() = new_tag_map;
        
        // 更新全局文件列表 (用于生成 Inode)
        let mut sorted_files: Vec<String> = unique_files.into_iter().collect();
        sorted_files.sort(); // 排序保证 Inode 相对稳定
        *self.global_file_list.lock().unwrap() = sorted_files;

        *last = SystemTime::now();
        println!(">>> [FUSE] Tags 视图刷新完成");
    }

    // 辅助：Tag 名字 -> Inode (1000 ~ 9999)
    fn tag_hash(&self, tag: &str) -> u64 {
        let mut h = 0u64;
        for b in tag.bytes() { h = h.wrapping_add(b as u64); }
        1000 + (h % 9000)
    }

    // 辅助：文件路径 -> Inode (100000 + index)
    fn file_path_to_inode(&self, path: &str) -> Option<u64> {
        let list = self.global_file_list.lock().unwrap();
        list.iter().position(|p| p == path).map(|i| 100000 + i as u64)
    }
}

const TTL: Duration = Duration::from_secs(1);
const NO_CACHE: Duration = Duration::from_secs(0);

impl Filesystem for AiFS {
    // ================= GETATTR =================
    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        let ts = UNIX_EPOCH;
        let dir_attr = FileAttr { ino, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::Directory, perm: 0o755, nlink: 2, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 };
        let file_attr = FileAttr { ino, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::RegularFile, perm: 0o666, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 };
        let link_attr = FileAttr { ino, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::Symlink, perm: 0o777, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 };

        match ino {
            1 => reply.attr(&TTL, &dir_attr), // Root
            2 => reply.attr(&TTL, &file_attr), // Ask
            3 => reply.attr(&NO_CACHE, &dir_attr), // Results
            4 => { // Answer.txt
                let answer = self.current_answer.lock().unwrap();
                let mut attr = file_attr;
                attr.size = answer.len() as u64;
                attr.perm = 0o444; // Read only
                reply.attr(&NO_CACHE, &attr);
            },
            10 => reply.attr(&TTL, &dir_attr), // /tags 目录
            5..=999 => { // Search Results Symlinks (5+)
                let index = (ino - 5) as usize;
                let results = self.current_results.lock().unwrap();
                if index < results.len() {
                    reply.attr(&NO_CACHE, &link_attr);
                } else {
                    reply.error(ENOENT);
                }
            },
            1000..=9999 => { // 具体的 Tag 目录
                self.refresh_tags_if_needed();
                // 这里其实应该校验一下 tag 是否存在，简便起见直接返回 Directory
                reply.attr(&TTL, &dir_attr);
            },
            _ => { // 100000+ Tag 下的文件
                if ino >= 100000 {
                    reply.attr(&TTL, &link_attr);
                } else {
                    reply.error(ENOENT);
                }
            }
        }
    }

    // ================= LOOKUP =================
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let ts = UNIX_EPOCH;
        let name_str = name.to_str().unwrap_or("");
        
        // 基础属性模板
        let dir_attr = |ino| FileAttr { ino, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::Directory, perm: 0o755, nlink: 2, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 };
        let file_attr = |ino, size| FileAttr { ino, size, blocks: 1, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::RegularFile, perm: 0o666, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 };
        let link_attr = |ino| FileAttr { ino, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::Symlink, perm: 0o777, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 };

        if parent == 1 { // Root
            match name_str {
                "ask" => reply.entry(&TTL, &file_attr(2, 0), 0),
                "results" => reply.entry(&NO_CACHE, &dir_attr(3), 0),
                "answer.txt" => {
                    let len = self.current_answer.lock().unwrap().len() as u64;
                    let mut attr = file_attr(4, len);
                    attr.perm = 0o444;
                    reply.entry(&NO_CACHE, &attr, 0);
                },
                "tags" => reply.entry(&TTL, &dir_attr(10), 0), // 新增 tags
                _ => reply.error(ENOENT),
            }
        } 
        else if parent == 3 { // /results
             if let Ok(idx) = name_str.parse::<u64>() {
                 let results = self.current_results.lock().unwrap();
                 if (idx as usize) < results.len() {
                    reply.entry(&NO_CACHE, &link_attr(idx + 5), 0);
                 } else {
                     reply.error(ENOENT);
                 }
             } else {
                 reply.error(ENOENT);
             }
        }
        else if parent == 10 { // /tags
            self.refresh_tags_if_needed();
            let cache = self.tag_cache.lock().unwrap();
            if cache.contains_key(name_str) {
                let ino = self.tag_hash(name_str);
                reply.entry(&TTL, &dir_attr(ino), 0);
            } else {
                reply.error(ENOENT);
            }
        }
        else if parent >= 1000 && parent < 10000 { // /tags/<tagname>
            // 在某个 tag 目录下查找文件
            self.refresh_tags_if_needed();
            
            // 这里比较麻烦，因为我们只知道文件名，需要反查它是否属于这个 tag
            // --- 第一阶段：持锁，只做查找和克隆 ---
            let files_copy: Option<Vec<String>> = {
                let cache = self.tag_cache.lock().unwrap();
                
                // 1. 找到对应的 Tag Name
                // 使用 find().cloned() 避免对 cache key 的引用
                let target_key = cache.keys()
                    .find(|key| self.tag_hash(key) == parent)
                    .cloned(); // 克隆出 String
                
                // 2. 如果找到了，克隆对应的文件列表
                if let Some(key) = target_key {
                    cache.get(&key).cloned() // 克隆出 Vec<String>
                } else {
                    None
                }
            }; 
            // 这里的花括号结束，cache 锁自动释放，不需要手动 drop()

            // --- 第二阶段：无锁，处理数据 ---
            if let Some(files) = files_copy {
                // 此时 files 是本地拥有的 Vec<String>，想怎么用怎么用
                let found_path = files.iter().find(|p| {
                     Path::new(p).file_name().and_then(|s| s.to_str()) == Some(name_str)
                });

                if let Some(path) = found_path {
                     // 再次获取锁 (file_path_to_inode 内部会锁 global_file_list)
                     // 因为上面的 tag_cache 锁已经释放，所以这里非常安全
                     if let Some(ino) = self.file_path_to_inode(path) {
                         reply.entry(&TTL, &link_attr(ino), 0);
                         return;
                     }
                }
            }
            reply.error(ENOENT);
        }
        else {
            reply.error(ENOENT);
        }
    }

    // ================= READDIR =================
    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        if ino == 1 { // Root
            if offset == 0 {
                let _ = reply.add(1, 0, FileType::Directory, ".");
                let _ = reply.add(1, 1, FileType::Directory, "..");
                let _ = reply.add(2, 2, FileType::RegularFile, "ask");
                let _ = reply.add(3, 3, FileType::Directory, "results");
                let _ = reply.add(4, 4, FileType::RegularFile, "answer.txt");
                let _ = reply.add(10, 5, FileType::Directory, "tags"); // 新增
            }
            reply.ok();
        } 
        else if ino == 3 { // Results
            if offset == 0 {
                let _ = reply.add(3, 0, FileType::Directory, ".");
                let _ = reply.add(3, 1, FileType::Directory, "..");
                let results = self.current_results.lock().unwrap();
                for (i, _) in results.iter().enumerate() {
                    let _ = reply.add(5 + i as u64, (i + 2) as i64, FileType::Symlink, &i.to_string());
                }
            }
            reply.ok();
        }
        else if ino == 10 { // /tags
            if offset == 0 {
                let _ = reply.add(10, 0, FileType::Directory, ".");
                let _ = reply.add(1, 1, FileType::Directory, "..");
                
                self.refresh_tags_if_needed();
                let cache = self.tag_cache.lock().unwrap();
                
                // 列出所有 tags
                let mut i = 2;
                for tag in cache.keys() {
                    let tag_ino = self.tag_hash(tag);
                    let _ = reply.add(tag_ino, i, FileType::Directory, tag);
                    i += 1;
                }
            }
            reply.ok();
        }
        else if ino >= 1000 && ino < 10000 { // /tags/<tagname>
            // 列出某个 tag 下的所有文件
            if offset == 0 {
                let _ = reply.add(ino, 0, FileType::Directory, ".");
                let _ = reply.add(10, 1, FileType::Directory, "..");

                self.refresh_tags_if_needed();
                let cache = self.tag_cache.lock().unwrap();
                
                // 反查 Tag Name
                let mut target_files = None;
                for (key, files) in cache.iter() {
                    if self.tag_hash(key) == ino {
                        target_files = Some(files.clone());
                        break;
                    }
                }
                
                drop(cache); // 释放锁

                if let Some(files) = target_files {
                    for (i, path) in files.iter().enumerate() {
                        // 获取文件名
                        if let Some(filename) = Path::new(path).file_name().and_then(|s| s.to_str()) {
                            // 获取全局 Inode
                            if let Some(file_ino) = self.file_path_to_inode(path) {
                                let _ = reply.add(file_ino, (i + 2) as i64, FileType::Symlink, filename);
                            }
                        }
                    }
                }
            }
            reply.ok();
        }
        else {
            reply.error(ENOENT);
        }
    }

    // ================= READLINK =================
    fn readlink(&mut self, _req: &Request, ino: u64, reply: ReplyData) {
        if ino >= 5 && ino < 1000 { // Search Results
             let index = (ino - 5) as usize;
             let results = self.current_results.lock().unwrap();
             if let Some(path) = results.get(index) {
                 reply.data(path.as_bytes());
             } else {
                 reply.error(ENOENT);
             }
        }
        else if ino >= 100000 { // Tag Files (全局文件列表)
             let index = (ino - 100000) as usize;
             let list = self.global_file_list.lock().unwrap();
             if let Some(path) = list.get(index) {
                 reply.data(path.as_bytes());
             } else {
                 reply.error(ENOENT);
             }
        }
        else {
            reply.error(ENOENT);
        }
    }

    // ================= READ (Answer.txt) =================
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

    // ================= SETATTR =================
    fn setattr(
        &mut self, _req: &Request, ino: u64, _mode: Option<u32>, _uid: Option<u32>, _gid: Option<u32>, _size: Option<u64>, 
        _atime: Option<fuser::TimeOrNow>, _mtime: Option<fuser::TimeOrNow>, _ctime: Option<std::time::SystemTime>, 
        _fh: Option<u64>, _crtime: Option<std::time::SystemTime>, _chgtime: Option<std::time::SystemTime>, 
        _bkuptime: Option<std::time::SystemTime>, _flags: Option<u32>, reply: ReplyAttr,
    ) {
        if ino == 2 {
            let ts = UNIX_EPOCH;
            let file_attr = FileAttr { ino: 2, size: 0, blocks: 0, atime: ts, mtime: ts, ctime: ts, crtime: ts, kind: FileType::RegularFile, perm: 0o666, nlink: 1, uid: 1000, gid: 1000, rdev: 0, flags: 0, blksize: 512 };
            reply.attr(&TTL, &file_attr);
        } else {
            reply.error(ENOENT);
        }
    }

    // ================= WRITE (Ask & RAG) =================
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
                 
                 // 注意：这里我们用 50 个结果做去重
                 let raw_paths = match embedding_result {
                     Ok(vec) => searcher.get_store().search("my_files", vec, 50).await.unwrap_or_default(),
                     Err(_) => vec![]
                 };

                 // 搜索层的去重逻辑
                 let mut unique_paths = Vec::new();
                 let mut seen = HashSet::new();
                 for p in raw_paths {
                    let normalized = p.replace("\\", "/");
                    if !seen.contains(&normalized) {
                        seen.insert(normalized.clone());
                        unique_paths.push(normalized);
                    }
                    if unique_paths.len() >= 10 { break; }
                 }

                 println!("   -> 2. 正在生成回答...");
                 let rag_answer = match searcher.ask_question(&query_clone).await {
                     Ok(ans) => ans,
                     Err(e) => format!("生成失败: {}", e),
                 };
                 
                 (unique_paths, rag_answer)
            });

            *results_store.lock().unwrap() = paths;
            *answer_store.lock().unwrap() = answer_text;
            
            println!(">>> [FUSE Write] 完成！");
            reply.written(data.len() as u32);
        } else {
            reply.error(EACCES);
        }
    }
}