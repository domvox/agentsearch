use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use super::{ItemChunk, ItemKind, Source, SourceItemMeta};

pub struct MoltisSource {
    jsonl_path: PathBuf,
}

impl MoltisSource {
    pub fn new(jsonl_path: PathBuf) -> Self {
        Self { jsonl_path }
    }
}

#[derive(Deserialize)]
struct MoltisEntry {
    content: Option<String>,
    created_at: Option<i64>,
    role: Option<String>,
    run_id: Option<String>,
    seq: Option<u32>,
    tool_name: Option<String>,
    arguments: Option<serde_json::Value>,
    result: Option<serde_json::Value>,
}

impl Source for MoltisSource {
    fn name(&self) -> &str {
        "moltis"
    }

    fn scan(&self) -> Result<Vec<SourceItemMeta>> {
        if !self.jsonl_path.exists() {
            return Ok(vec![]);
        }
        let meta = std::fs::metadata(&self.jsonl_path)?;
        let size = meta.len();
        let mtime = meta.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs()).unwrap_or(0);

        // Parse run_ids to enumerate sessions
        let file = std::fs::File::open(&self.jsonl_path)?;
        let reader = BufReader::new(file);
        let mut run_ids: Vec<String> = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() { continue; }
            if let Ok(entry) = serde_json::from_str::<MoltisEntry>(&line) {
                if let Some(rid) = entry.run_id {
                    if !run_ids.contains(&rid) {
                        run_ids.push(rid);
                    }
                }
            }
        }

        Ok(run_ids.into_iter().map(|rid| SourceItemMeta {
            item_id: rid,
            fingerprint: format!("{}:{}", size, mtime),
        }).collect())
    }

    fn load(&self, item_id: &str) -> Result<Vec<ItemChunk>> {
        let file = std::fs::File::open(&self.jsonl_path)?;
        let reader = BufReader::new(file);
        let mut messages: Vec<(String, String, i64, u32)> = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() { continue; }
            let entry: MoltisEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.run_id.as_deref() != Some(item_id) { continue; }

            let role = entry.role.unwrap_or_default();
            let seq = entry.seq.unwrap_or(0);
            let ts = entry.created_at.unwrap_or(0);

            let content = match role.as_str() {
                "tool_result" => {
                    let name = entry.tool_name.as_deref().unwrap_or("unknown");
                    let result_str = entry.result
                        .map(|v| {
                            let s = v.to_string();
                            if s.len() > 2048 {
                                let mut end = 2048;
                                while !s.is_char_boundary(end) { end -= 1; }
                                format!("{}...[truncated]", &s[..end])
                            } else { s }
                        })
                        .unwrap_or_default();
                    format!("[tool:{}] {}", name, result_str)
                }
                _ => entry.content.unwrap_or_default(),
            };

            messages.push((role, content, ts, seq));
        }

        messages.sort_by_key(|m| (m.3, m.2));

        // Chunk by user turns
        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut chunk_ts = 0i64;
        let mut ordinal = 0u32;

        for (role, content, ts, _) in &messages {
            if role == "user" && !current.is_empty() {
                chunks.push(make_chunk(item_id, ordinal, chunk_ts, &current));
                current.clear();
                ordinal += 1;
            }
            if chunk_ts == 0 { chunk_ts = *ts; }
            current.push_str(&format!("{}: {}\n\n", role, content));
        }
        if !current.is_empty() {
            chunks.push(make_chunk(item_id, ordinal, chunk_ts, &current));
        }

        Ok(chunks)
    }
}

fn make_chunk(item_id: &str, ordinal: u32, ts: i64, content: &str) -> ItemChunk {
    ItemChunk {
        item_id: item_id.to_string(),
        chunk_id: format!("moltis:{}:{}", item_id, ordinal),
        source: "moltis".into(),
        kind: ItemKind::Session,
        title: None,
        timestamp: ts,
        ordinal,
        content: content.to_string(),
        role: Some("user".into()),
        path: None,
    }
}
