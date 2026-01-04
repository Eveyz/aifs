// src/brain/store.rs
use lancedb::{connect, Connection};
use arrow_array::{FixedSizeListArray, RecordBatch, RecordBatchIterator, Float32Array, StringArray, Array};
// use arrow_array::types::Float32Type;
use arrow_schema::{Schema, Field, DataType};
use lancedb::query::{ExecutableQuery, QueryBase};
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

    pub async fn add_documents(
        &self, 
        table_name: &str, 
        paths: Vec<String>, 
        vectors: Vec<Vec<f32>>, 
        tags_list: Vec<String>, 
        dim: i32
    ) -> Result<()> {
        if paths.is_empty() { return Ok(()); }

        // 1. 定义 Schema
        let schema = Arc::new(Schema::new(vec![
            Field::new("path", DataType::Utf8, false),
            Field::new("tags", DataType::Utf8, false),
            Field::new("vector", DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dim,
            ), false),
        ]));

        // 2. 准备 Arrow Array
        let path_array: Arc<dyn Array> = Arc::new(StringArray::from(paths));
        let tags_array: Arc<dyn Array> = Arc::new(StringArray::from(tags_list));

        // Flatten vectors and build Float32Array
        let flat_vectors: Vec<f32> = vectors.into_iter().flatten().collect();
        let values = Arc::new(Float32Array::from_iter(flat_vectors.into_iter().map(Some)));

        // Build FixedSizeListArray
        let vector_array = Arc::new(FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            dim,
            values,
            None,
        )?) as _;

        // 3. 构建 RecordBatch
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                path_array,
                tags_array,
                vector_array,
            ],
        )?;

        // 4. Wrap in RecordBatchIterator
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());

        // 5. 写入数据库
        let table_exists = self.conn.open_table(table_name).execute().await.is_ok();

        if table_exists {
            let tbl = self.conn.open_table(table_name).execute().await?;
            tbl.add(Box::new(batches)).execute().await?;
        } else {
            self.conn.create_table(table_name, Box::new(batches)).execute().await?;
        }

        Ok(())
    }

    pub async fn search(&self, table_name: &str, query_vec: Vec<f32>, limit: usize) -> Result<Vec<String>> {
        let table = self.conn.open_table(table_name).execute().await?;
        
        let results = table.query()
            .nearest_to(query_vec)? 
            .limit(limit)
            .execute()
            .await?;
            
        let batches: Vec<RecordBatch> = results.try_collect().await?;
        let mut paths = Vec::new();

        for batch in batches {
            let path_col = batch.column(0).as_any().downcast_ref::<StringArray>().unwrap();
            for i in 0..path_col.len() {
                paths.push(path_col.value(i).to_string());
            }
        }
        Ok(paths)
    }

    pub async fn get_all_files_with_tags(&self, table_name: &str) -> Result<Vec<(String, String)>> {
        if self.conn.open_table(table_name).execute().await.is_err() {
            return Ok(Vec::new());
        }

        let table = self.conn.open_table(table_name).execute().await?;
        
        let stream = table.query().limit(10000).execute().await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        
        let mut results = Vec::new();

        for batch in batches {
            let paths = batch.column(0).as_any().downcast_ref::<StringArray>().unwrap();
            let tags = batch.column(1).as_any().downcast_ref::<StringArray>().unwrap();
            
            for i in 0..paths.len() {
                results.push((
                    paths.value(i).to_string(), 
                    tags.value(i).to_string()
                ));
            }
        }
        Ok(results)
    }
}
