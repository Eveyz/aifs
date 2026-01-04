// src/brain/indexer.rs
use std::fs::{self, File};
use std::io::Read;
use walkdir::WalkDir;
use crate::brain::store::VectorStore;
use crate::brain::ollama::OllamaClient;
use anyhow::Result;
use std::path::Path;
use zip::ZipArchive;

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

    async fn get_embedding_safe(&self, text: &str) -> Option<Vec<f32>> {
        let mut current_text = text;
        let mut attempts = 0;
        
        while attempts < 3 {
            match self.client.get_embedding(current_text).await {
                Ok(vec) => return Some(vec),
                Err(e) => {
                    attempts += 1;
                    let new_len = current_text.len() / 2;
                    if new_len == 0 { break; }
                    current_text = &current_text[0..new_len];
                }
            }
        }
        None
    }

    fn extract_text(&self, path: &Path) -> Result<String> {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        
        match ext.to_lowercase().as_str() {
            "rs" | "md" | "txt" | "toml" | "json" => {
                Ok(fs::read_to_string(path)?)
            },
            "docx" | "pptx" => {
                let file = File::open(path)?;
                let mut archive = ZipArchive::new(file)?;
                let mut content = String::new();
                for i in 0..archive.len() {
                    let mut file = archive.by_index(i)?;
                    let name = file.name().to_string();
                    if name.contains("word/document.xml") || name.contains("ppt/slides/slide") {
                        let mut xml_content = String::new();
                        file.read_to_string(&mut xml_content)?;
                        let text_parts: Vec<&str> = xml_content.split('>').collect();
                        for part in text_parts {
                            if let Some(end) = part.find('<') {
                                let text = &part[0..end];
                                if !text.trim().is_empty() {
                                    content.push_str(text);
                                    content.push('\n');
                                }
                            }
                        }
                    }
                }
                Ok(content)
            },
            _ => Ok(String::new()), 
        }
    }

    // 调用 LLM 生成 Tags
    async fn generate_tags(&self, path: &str, content: &str) -> String {
        let snippet = if content.len() > 2000 { &content[0..2000] } else { content };
        
        let prompt = format!(
            "Analyze the following code/text and generate exactly 1 to 3 categorization tags (e.g., 'networking', 'ui', 'database', 'utils').
            Return ONLY the tags separated by commas. Do not explain.
            File Path: {}
            Content:
            {}", 
            path, snippet
        );

        match self.client.generate(&prompt).await {
            Ok(res) => {
                let clean = res.replace("\n", "").replace(".", "").to_lowercase();
                println!("   [AutoTag] {} -> [{}]", path, clean);
                clean
            },
            Err(_) => "misc".to_string()
        }
    }

    pub async fn index_folder(&self, folder_path: &str) -> Result<()> {
        println!(">>> 使用 Ollama 扫描: {}", folder_path);
        
        let mut valid_paths = Vec::new();
        let mut valid_vectors = Vec::new();
        let mut valid_tags = Vec::new(); // 存放 Tags
        let mut target_dim: Option<usize> = None;

        for entry in WalkDir::new(folder_path).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                let path = entry.path();
                let path_str = path.to_string_lossy().to_string();
                
                let content = match self.extract_text(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                if content.trim().is_empty() { continue; }

                // 1. 生成 Tags
                let tags = self.generate_tags(&path_str, &content).await;

                let max_len = 6000;
                let snippet = if content.len() > max_len { &content[0..max_len] } else { &content };
                let final_text = format!("filepath: {}\ntags: {}\n---\n{}", path_str, tags, snippet);

                // 2. Embedding
                if let Some(vec) = self.get_embedding_safe(&final_text).await {
                    let current_dim = vec.len();
                    if current_dim == 0 { continue; }

                    match target_dim {
                        None => {
                            // --- 修复点：第一个文件也要 push tags ---
                            target_dim = Some(current_dim);
                            println!("\n>>> 锁定模型维度为: {}", current_dim);
                            valid_paths.push(path_str.clone());
                            valid_vectors.push(vec);
                            valid_tags.push(tags); // <--- 之前漏了这行！！！
                            print!("+"); 
                        }
                        Some(dim) => {
                            if current_dim == dim {
                                valid_paths.push(path_str.clone());
                                valid_vectors.push(vec);
                                valid_tags.push(tags); // 后续文件 push tags
                                print!(".");
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

        if valid_paths.is_empty() {
            println!("\n没有有效数据可存入。");
            return Ok(());
        }

        let final_dim = target_dim.unwrap_or(0); 
        println!("\n>>> 正在存入 LanceDB (共 {} 个文件)...", valid_paths.len());
        
        // 现在三个 vector 长度一定一致了
        self.store.add_documents("my_files", valid_paths, valid_vectors, valid_tags, final_dim as i32).await?; 
        
        println!(">>> 完成！");
        Ok(())
    }

    // 单文件索引逻辑也同步加上 tags
    pub async fn index_file(&self, path_str: &str) -> Result<()> {
        let content = match fs::read_to_string(path_str) {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };
        if content.trim().is_empty() { return Ok(()); }
        
        let tags = self.generate_tags(path_str, &content).await;

        let max_len = 6000;
        let snippet = if content.len() > max_len { &content[0..max_len] } else { &content };
        let final_text = format!("filepath: {}\ntags: {}\n---\n{}", path_str, tags, snippet);

        if let Some(vec) = self.get_embedding_safe(&final_text).await {
             let dim = vec.len();
             if dim > 0 {
                 println!(">>> [热更新] 索引成功: {}", path_str);
                 self.store.add_documents("my_files", vec![path_str.to_string()], vec![vec], vec![tags], dim as i32).await?;
             }
        }
        Ok(())
    }
}