// src/brain/ollama.rs
use serde::{Deserialize, Serialize};
use anyhow::Result;

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

pub struct OllamaClient {
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OllamaClient {
    pub fn new(model: &str) -> Self {
        Self {
            base_url: "http://172.30.240.1:11434/api/embeddings".to_string(),
            model: model.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn get_embedding(&self, text: &str) -> Result<Vec<f32>> {
        let request = EmbeddingRequest {
            model: self.model.clone(),
            prompt: text.to_string(),
        };

        let res = self.client
            .post(&self.base_url)
            .json(&request)
            .send()
            .await?;

        if !res.status().is_success() {
            return Err(anyhow::anyhow!("Ollama API error: {}", res.status()));
        }

        let data: EmbeddingResponse = res.json().await?;
        Ok(data.embedding)
    }
}