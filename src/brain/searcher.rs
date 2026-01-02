// src/brain/searcher.rs
use crate::brain::store::VectorStore;
use crate::brain::ollama::OllamaClient;
use anyhow::Result;
use std::fs;
use std::collections::HashSet;

pub struct Searcher {
    client: OllamaClient,
    store: VectorStore,
}

impl Searcher {
    pub async fn new(db_uri: &str, model_name: &str) -> Self {
        let client = OllamaClient::new(model_name);
        let store = VectorStore::new(db_uri).await;
        Self { client, store }
    }

    pub async fn search(&self, query_text: &str) -> Result<()> {
        println!(">>> 正在思考: '{}' ...", query_text);

        // 1. 把你的问题变成向量
        let query_vector = self.client.get_embedding(query_text).await?;

        // 2. 去数据库找最相似的 5 个文件
        let results = self.store.search("my_files", query_vector, 5).await?;

        if results.is_empty() {
            println!("没有找到相关结果。");
        } else {
            println!("\n>>> 找到最相关的 5 个文件:");
            for (i, path) in results.iter().enumerate() {
                println!("{}. {}", i + 1, path);
            }
        }

        Ok(())
    }
    
    pub fn get_client(&self) -> &OllamaClient {
        &self.client
    }

    pub fn get_store(&self) -> &VectorStore {
        &self.store
    }

    // --- 新增：RAG 核心逻辑 ---
    pub async fn ask_question(&self, query: &str) -> Result<String> {
        // 1. 先搜索最相关的 3 个文件 (只要最相关的，防止上下文太长)
        let query_vec = self.client.get_embedding(query).await?;
        let raw_paths = self.store.search("my_files", query_vec, 50).await?;
        
        if raw_paths.is_empty() {
            return Ok("没有找到相关文件，无法回答。".to_string());
        }

        // --- 修改点 2：强力去重逻辑 ---
        let mut unique_paths = Vec::new();
        let mut seen = HashSet::new();

        for path in raw_paths {
            // 简单的路径标准化，防止 ./src/main.rs 和 src/main.rs 被当成两个
            // 顺便把反斜杠修了
            let normalized = path.replace("\\", "/"); 
            
            if !seen.contains(&normalized) {
                seen.insert(normalized.clone());
                unique_paths.push(path); // 注意：这里还是存原始路径去读取文件
            }
            
            // 凑够 10 个就收工
            if unique_paths.len() >= 10 {
                break;
            }
        }

        if unique_paths.len() == 0 {
            return Ok("没有找到相关文件，无法回答。".to_string());
        }

        println!(">>> [RAG Debug] AI 正在阅读以下文件:");
        for p in &unique_paths {
            println!("   - {}", p);
        }

        // 2. 读取这些文件的内容
        let mut context = String::new();
        for path in unique_paths {
            // 这里要做路径修正，防止 Windows/Linux 路径问题
            // 简单起见，我们假设现在运行在正确环境下
            if let Ok(content) = fs::read_to_string(&path) {
                // 限制每个文件读取长度，防止爆显存
                let snippet = if content.len() > 2000 { &content[0..2000] } else { &content };
                context.push_str(&format!("\n--- File: {} ---\n{}\n", path, snippet));
            }
        }

        // 3. 构建 Prompt
        let prompt = format!(
            "你是一个代码助手。请根据以下代码片段回答用户的问题。\n\n上下文代码:\n{}\n\n用户问题: {}\n\n请用中文直接回答，不要啰嗦。",
            context, query
        );

        // 4. 调用 LLM 生成回答
        println!(">>> [RAG] 正在向 LLM 提问，上下文长度: {}", context.len());
        let answer = self.client.generate(&prompt).await?;
        
        Ok(answer)
    }

}