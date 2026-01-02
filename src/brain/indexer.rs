// src/brain/indexer.rs
use std::fs;
use walkdir::WalkDir;
use crate::brain::store::VectorStore;
use crate::brain::ollama::OllamaClient;
use anyhow::Result;

pub struct Indexer {
    client: OllamaClient,
    store: VectorStore,
}

impl Indexer {
    pub async fn new(db_uri: &str, model_name: &str) -> Self {
        let client = OllamaClient::new(model_name);
        let store = VectorStore::new(db_uri).await;
        Self { client, store }
    }

    // 递归截断重试逻辑
    async fn get_embedding_safe(&self, text: &str) -> Option<Vec<f32>> {
        let mut current_text = text;
        let mut attempts = 0;
        
        // 最多重试 3 次，每次砍一半
        while attempts < 3 {
            match self.client.get_embedding(current_text).await {
                Ok(vec) => return Some(vec),
                Err(e) => {
                    attempts += 1;
                    let new_len = current_text.len() / 2;
                    println!("   [Warn] Embedding 失败 (长度 {}), 正在重试截断到 {}... 错误: {}", current_text.len(), new_len, e);
                    if new_len == 0 { break; }
                    current_text = &current_text[0..new_len];
                }
            }
        }
        println!("   [Error] 最终放弃该文件，无法生成向量。");
        None
    }

    pub async fn index_folder(&self, folder_path: &str) -> Result<()> {
        println!(">>> 使用 Ollama 扫描: {}", folder_path);
        
        let mut valid_paths = Vec::new();
        let mut valid_vectors = Vec::new();
        let mut target_dim: Option<usize> = None;

        for entry in WalkDir::new(folder_path).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                let path_str = entry.path().to_string_lossy().to_string();
                
                if path_str.ends_with(".rs") || path_str.ends_with(".md") || path_str.ends_with(".toml") {
                    
                    let content = match fs::read_to_string(&path_str) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    if content.trim().is_empty() { continue; }

                    // 1. 更加激进的长度限制，防止 500 错误
                    // 虽然 Nomic 支持长文本，但 Ollama 本地跑有时候显存不够
                    // 我们先限制到 6000 字符，通常够用了
                    let max_len = 6000;
                    let snippet = if content.len() > max_len { &content[0..max_len] } else { &content };
                    
                    // 2. Prompt Engineering: 把文件名显式加进去
                    // 这样搜 "fs.rs" 或 "PollWatcher" 时，文件名能提供很大权重
                    let final_text = format!("filepath: {}\n---\n{}", path_str, snippet);

                    // 3. 调用安全的 embedding 方法
                    if let Some(vec) = self.get_embedding_safe(&final_text).await {
                        let current_dim = vec.len();
                        if current_dim == 0 { continue; }

                        match target_dim {
                            None => {
                                target_dim = Some(current_dim);
                                println!("\n>>> 锁定模型维度为: {}", current_dim);
                                valid_paths.push(path_str.clone());
                                valid_vectors.push(vec);
                                print!("+"); // 第一文件
                            }
                            Some(dim) => {
                                if current_dim == dim {
                                    valid_paths.push(path_str.clone());
                                    valid_vectors.push(vec);
                                    print!("."); // 成功
                                } else {
                                    println!("\n[剔除] 维度不匹配: {}", path_str);
                                }
                            }
                        }
                        use std::io::Write;
                        std::io::stdout().flush().unwrap();
                    }
                }
            }
        }

        if valid_paths.is_empty() {
            println!("\n没有有效数据可存入。");
            return Ok(());
        }

        let final_dim = target_dim.unwrap_or(0); 
        println!("\n>>> 正在存入 LanceDB (共 {} 个文件)...", valid_paths.len());
        self.store.add_documents("my_files", valid_paths, valid_vectors, final_dim as i32).await?; 
        println!(">>> 完成！");
        Ok(())
    }

    // 单文件索引逻辑也同步修改
    pub async fn index_file(&self, path_str: &str) -> Result<()> {
        let content = match fs::read_to_string(path_str) {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };
        if content.trim().is_empty() { return Ok(()); }

        let max_len = 6000;
        let snippet = if content.len() > max_len { &content[0..max_len] } else { &content };
        let final_text = format!("filepath: {}\n---\n{}", path_str, snippet);

        if let Some(vec) = self.get_embedding_safe(&final_text).await {
             let dim = vec.len();
             if dim > 0 {
                 println!(">>> [热更新] 索引成功: {}", path_str);
                 self.store.add_documents("my_files", vec![path_str.to_string()], vec![vec], dim as i32).await?;
             }
        }
        Ok(())
    }

}