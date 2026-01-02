// src/brain/searcher.rs
use crate::brain::store::VectorStore;
use crate::brain::ollama::OllamaClient;
use anyhow::Result;

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

}