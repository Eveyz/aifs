// src/brain/store.rs
use lancedb::{connect, Connection};
use lancedb::query::{ExecutableQuery, QueryBase}; 
use arrow::array::{RecordBatch, RecordBatchIterator, StringArray, FixedSizeListArray, Float32Array};
use arrow::datatypes::{DataType, Field, Schema};
// --- 关键修正：确保引入 Array trait ---
use arrow::array::Array; 
use std::sync::Arc;
use anyhow::Result;
use futures::TryStreamExt; 

pub struct VectorStore {
    conn: Connection,
}

impl VectorStore {
    pub async fn new(uri: &str) -> Self {
        let conn = connect(uri).execute().await.unwrap();
        Self { conn }
    }

    // 存入数据
    pub async fn add_documents(&self, table_name: &str, paths: Vec<String>, vectors: Vec<Vec<f32>>, dim: i32) -> Result<()> {
        if paths.is_empty() { return Ok(()); }

        let schema = Arc::new(Schema::new(vec![
            Field::new("path", DataType::Utf8, false),
            Field::new("vector", DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dim,
            ), false),
        ]));

        let path_array = StringArray::from(paths);
        let flat_vectors: Vec<f32> = vectors.iter().flatten().copied().collect();
        let vector_values = Float32Array::from(flat_vectors);
        let vector_array = FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            dim,
            Arc::new(vector_values),
            None,
        )?;

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(path_array), Arc::new(vector_array)],
        )?;

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());

        if self.conn.open_table(table_name).execute().await.is_ok() {
             let _ = self.conn.open_table(table_name).execute().await?.add(batches).execute().await?;
        } else {
            let _ = self.conn.create_table(table_name, batches).execute().await?;
        }
        
        Ok(())
    }

    // 语义搜索
    pub async fn search(&self, table_name: &str, query_vector: Vec<f32>, limit: usize) -> Result<Vec<String>> {
        let table = self.conn.open_table(table_name).execute().await?;

        let mut stream = table
            .query()
            .nearest_to(query_vector)? 
            .limit(limit)
            .execute() 
            .await?;

        let mut results = Vec::new();

        while let Some(batch) = stream.try_next().await? {
            let path_column = batch.column_by_name("path")
                .ok_or(anyhow::anyhow!("Column 'path' not found"))?;
            
            if let Some(strings) = path_column.as_any().downcast_ref::<StringArray>() {
                // --- 修正点：显式调用 trait 方法，防止编译器发懵 ---
                let len = Array::len(strings); 
                for i in 0..len {
                    results.push(strings.value(i).to_string());
                }
            }
        }

        Ok(results)
    }
}