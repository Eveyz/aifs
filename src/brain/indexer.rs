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

    pub async fn index_folder(&self, folder_path: &str) -> Result<()> {
        println!(">>> 使用 Ollama 扫描: {}", folder_path);
        
        let mut valid_paths = Vec::new();
        let mut valid_vectors = Vec::new();
        
        // 核心变量：用于锁定标准维度
        let mut target_dim: Option<usize> = None;

        for entry in WalkDir::new(folder_path).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                let path_str = entry.path().to_string_lossy().to_string();
                
                // 1. 基础过滤：只看文本文件
                // TODO: 可配置化 支持更多文件类型
                if path_str.ends_with(".rs") || path_str.ends_with(".md") || path_str.ends_with(".toml") || path_str.ends_with(".txt") {
                    
                    // 2. 读取内容
                    let content = match fs::read_to_string(&path_str) {
                        Ok(c) => c,
                        Err(_) => continue, // 读失败直接跳过
                    };

                    // 3. 空内容直接跳过，省一次 HTTP 请求
                    if content.trim().is_empty() { 
                        continue; 
                    }

                    // 简单的截断，防止 Token 溢出
                    let snippet = if content.len() > 1000 { &content[0..1000] } else { &content };
                    
                    // 4. 获取向量
                    match self.client.get_embedding(snippet).await {
                        Ok(vec) => {
                            let current_dim = vec.len();

                            // 5. 维度一致性检查 (Robust Logic)
                            if current_dim == 0 {
                                println!("\n[跳过] 空向量: {}", path_str);
                                continue;
                            }

                            match target_dim {
                                // A. 如果还没确定标准维度，就把第一个非零维度定为标准
                                None => {
                                    target_dim = Some(current_dim);
                                    println!("\n>>> 锁定模型维度为: {}", current_dim);
                                    valid_paths.push(path_str);
                                    valid_vectors.push(vec);
                                }
                                // B. 如果已经确定了标准，就检查是否一致
                                Some(dim) => {
                                    if current_dim == dim {
                                        valid_paths.push(path_str);
                                        valid_vectors.push(vec);
                                        print!("."); 
                                        use std::io::Write;
                                        std::io::stdout().flush().unwrap();
                                    } else {
                                        println!("\n[剔除] 维度不匹配 {}: 期望 {}, 实际 {}", path_str, dim, current_dim);
                                    }
                                }
                            }
                        }
                        Err(e) => println!("\n[Error] Ollama 请求失败 {}: {}", path_str, e),
                    }
                }
            }
        }

        if valid_paths.is_empty() {
            println!("\n没有有效数据可存入。");
            return Ok(());
        }

        // 解包维度，如果 target_dim 是 None 说明全是空的，但前面 valid_paths check 已经拦住了
        let final_dim = target_dim.unwrap_or(0); 

        println!("\n>>> 正在存入 LanceDB (共 {} 个文件)...", valid_paths.len());
        
        self.store.add_documents("my_files", valid_paths, valid_vectors, final_dim as i32).await?; 
        
        println!(">>> 完成！");
        Ok(())
    }

    // 新增：只索引单个文件
    pub async fn index_file(&self, path_str: &str) -> Result<()> {
        // 1. 读取内容
        let content = match fs::read_to_string(path_str) {
            Ok(c) => c,
            Err(_) => return Ok(()), // 读不到就算了（可能是删除了）
        };

        if content.trim().is_empty() { return Ok(()); }
        let snippet = if content.len() > 1000 { &content[0..1000] } else { &content };

        // 2. 生成向量
        match self.client.get_embedding(snippet).await {
            Ok(vec) => {
                let dim = vec.len();
                if dim > 0 {
                    // 3. 这里的逻辑稍微有点硬：
                    // LanceDB 目前 append 比较容易，update/delete 比较麻烦
                    // MVP 方案：不管旧的，直接 append 新的。
                    // (这会导致数据膨胀，但作为 Demo 先跑通再说，之后我们加去重逻辑)
                    println!(">>> [热更新] 正在重写索引: {}", path_str);
                    self.store.add_documents("my_files", vec![path_str.to_string()], vec![vec], dim as i32).await?;
                }
            }
            Err(e) => println!("[Error] Embedding failed: {}", e),
        }
        Ok(())
    }
    
}