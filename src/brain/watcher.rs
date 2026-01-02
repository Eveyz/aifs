// src/brain/watcher.rs
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc::channel;
// use std::time::Duration;
use crate::brain::indexer::Indexer;
use std::sync::Arc;
use tokio::runtime::Handle; // 关键：我们需要在同步回调里调异步代码

pub async fn start_watching(folder_path: String, indexer: Arc<Indexer>) {
    tokio::task::spawn_blocking(move || {
        println!(">>> 启动文件监控: {}", folder_path);

        // 1. 创建一个通道来接收文件变动事件
        let (tx, rx) = channel();

        // 2. 初始化 Watcher
        let mut watcher = RecommendedWatcher::new(tx, Config::default()).unwrap();

        // 3. 开始递归监控
        watcher.watch(Path::new(&folder_path), RecursiveMode::Recursive).unwrap();

        // 4. 获取当前的 Tokio Handle (因为 notify 是同步的，indexer 是异步的)
        let rt_handle = Handle::current();

        // 5. 循环处理事件
        for res in rx {
            match res {
                Ok(event) => {
                    // 只处理内容修改(Modify)和新增(Create)
                    // 注意：真实 OS 处理由于编辑器原子写入，逻辑会更复杂，这里是 MVP
                    match event.kind {
                        notify::EventKind::Modify(_) | notify::EventKind::Create(_) => {
                            for path in event.paths {
                                let path_str = path.to_string_lossy().to_string();
                                if path_str.ends_with(".rs") || path_str.ends_with(".md") {
                                    println!(">>> 检测到文件变动: {}", path_str);
                                    
                                    // 重新索引单个文件
                                    // 这里我们需要给 Indexer 加一个 index_single_file 方法
                                    let idx = indexer.clone();
                                    rt_handle.block_on(async {
                                        let _ = idx.index_file(&path_str).await;
                                    });
                                }
                            }
                        },
                        _ => {}
                    }
                },
                Err(e) => println!("watch error: {:?}", e),
            }
        }
    });
}