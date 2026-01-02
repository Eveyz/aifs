// src/brain/ollama.rs
use serde::{Deserialize, Serialize};
use anyhow::Result;
use reqwest::Client;
use std::env;

// 定义 Ollama 的请求格式
#[derive(Serialize)]
struct EmbeddingRequest {
    model: String,
    prompt: String,
}

// 定义 Ollama 的响应格式
#[derive(Deserialize)]
struct EmbeddingResponse {
    embedding: Vec<f32>,
}

#[derive(Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    stream: bool,
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

#[derive(Clone)]
pub struct OllamaClient {
    base_url: String,     // 原来的 embedding url
    generate_url: String, // 新增：generate url
    model: String,        // embedding 模型 (nomic)
    chat_model: String,   // 新增：聊天模型 (llama3)
    client: Client,
}

impl OllamaClient {
    pub fn new(model: &str) -> Self {
        let host = env::var("OLLAMA_HOST").unwrap_or_else(|_| "172.30.240.1".to_string());
        
        Self {
            base_url: format!("http://{}:11434/api/embeddings", host),
            generate_url: format!("http://{}:11434/api/generate", host),
            model: model.to_string(),
            chat_model: "qwen3:8b".to_string(), 
            client: Client::new(),
        }
    }

    pub async fn get_embedding(&self, text: &str) -> Result<Vec<f32>> {
        let req = EmbeddingRequest {
            model: self.model.clone(),
            prompt: text.to_string(),
        };

        let res = self.client.post(&self.base_url)
            .json(&req)
            .send()
            .await?
            .json::<EmbeddingResponse>()
            .await?;

        Ok(res.embedding)
    }

    // --- 新增：生成回答 ---
    pub async fn generate(&self, prompt: &str) -> Result<String> {
        let req = GenerateRequest {
            model: self.chat_model.clone(), // 使用聊天模型
            prompt: prompt.to_string(),
            stream: false, // 简化处理，不用流式
        };

        let res = self.client.post(&self.generate_url)
            .json(&req)
            .send()
            .await?;
            
        if !res.status().is_success() {
            return Err(anyhow::anyhow!("Ollama generate failed: {}", res.status()));
        }

        let body = res.json::<GenerateResponse>().await?;
        Ok(body.response)
    }
    
}