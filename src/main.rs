mod brain;
use brain::indexer::Indexer;
use brain::searcher::Searcher;
use brain::watcher::start_watching; // 引入
use std::sync::Arc;
use std::env;
mod fs;
use fs::AiFS;
use fuser::MountOption;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 确保你的 Ollama 里有这个模型: ollama pull nomic-embed-text
    let model_name = "nomic-embed-text:v1.5"; 
    let db_uri = "data/lancedb_store";
    
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        println!("使用方法:");
        println!("  1. 建立索引: cargo run -- index <文件夹路径>");
        println!("  2. 语义搜索: cargo run -- search <你的问题>");
        return Ok(());
    }

    let command = &args[1];
    let parameter = &args[2];

    match command.as_str() {
        "index" => {
            let indexer = Indexer::new(db_uri, model_name).await;
            indexer.index_folder(parameter).await?;
        }
        "search" => {
            let searcher = Searcher::new(db_uri, model_name).await;
            searcher.search(parameter).await?;
        }
        // AI Test Update
        "mount" => {
            let mountpoint = parameter.to_string();
            println!("正在挂载 AI 文件系统到: {}", mountpoint);
            println!("请在另一个终端尝试: echo '你的问题' > {}/ask", mountpoint);
            
            let searcher = Searcher::new(db_uri, model_name).await;
            // 注意：这里为了方便，我们重新 new 了一个 indexer，实际应该共享连接
            let indexer = Arc::new(Indexer::new(db_uri, model_name).await);

            // 2. 启动后台监控 (假设监控 ./src 目录，你可以改成参数传入)
            let watch_target = "./src".to_string(); // 这里先写死，或者用 env::current_dir()
            start_watching(watch_target, indexer).await;

            let fs = AiFS::new(searcher);
            
            let mount_handle = tokio::task::spawn_blocking(move || {
                fuser::mount2(fs, mountpoint, &[
                    MountOption::RW, 
                    MountOption::FSName("aifs".to_string()),
                    MountOption::AutoUnmount,
                ]).unwrap();
            });

            // 等待挂载结束 (实际上如果不卸载，这里会一直等)
            mount_handle.await.unwrap();
        }
        _ => {
            println!("未知命令: {}", command);
        }
    }

    Ok(())
}