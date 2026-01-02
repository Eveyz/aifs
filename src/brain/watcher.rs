// src/brain/watcher.rs
use notify::{Config, PollWatcher, RecursiveMode, Watcher}; // 改用 PollWatcher
use std::path::Path;
use std::sync::mpsc::channel;
use std::time::Duration;
use crate::brain::indexer::Indexer;
use std::sync::Arc;
use tokio::runtime::Handle;

pub async fn start_watching(folder_path: String, indexer: Arc<Indexer>) {
    tokio::task::spawn_blocking(move || {
        println!(">>> 启动文件监控 (轮询模式): {}", folder_path);

        let (tx, rx) = channel();

        // --- 关键修改：强制使用轮询 (PollWatcher) ---
        // 每 2 秒检查一次。虽然慢一点点，但绝对兼容 WSL 和 Windows 挂载盘
        let config = Config::default().with_poll_interval(Duration::from_secs(2));
        let mut watcher = PollWatcher::new(tx, config).unwrap();
        // ------------------------------------------

        watcher.watch(Path::new(&folder_path), RecursiveMode::Recursive).unwrap();

        let rt_handle = Handle::current();

        for res in rx {
            match res {
                Ok(event) => {
                    // 打印原始事件，看看你的编辑器到底干了啥
                    println!("DEBUG: 收到事件 {:?}", event.kind); 

                    // 放宽条件：只要有变动涉及 .rs/.md 就处理
                    // 不管是 Modify, Create, 还是 Rename
                    for path in event.paths {
                        let path_str = path.to_string_lossy().to_string();
                        if path_str.ends_with(".rs") || path_str.ends_with(".md") {
                            println!(">>> 检测到文件变动: {}", path_str);
                            
                            let idx = indexer.clone();
                            rt_handle.block_on(async {
                                let _ = idx.index_file(&path_str).await;
                            });
                        }
                    }
                },
                Err(e) => println!("watch error: {:?}", e),
            }
        }
    });
}