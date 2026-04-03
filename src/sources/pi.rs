use anyhow::Result;
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use super::{ItemChunk, ItemKind, Source, SourceItemMeta};

/// Pi coding agent sessions (~/.pi/agent/sessions/*/*.jsonl)
pub struct PiSource {
    sessions_dir: PathBuf,
}

impl PiSource {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }
}

#[derive(Deserialize)]
struct PiLine {
    #[serde(rename = "type")]
    event_type: Option<String>,
    id: Option<String>,
    timestamp: Option<String>,
    message: Option<PiMessage>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct PiMessage {
    role: Option<String>,
    content: Option<serde_json::Value>,
}

fn extract_text_content(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|item| {
                if item.get("type")?.as_str()? == "text" {
                    item.get("text")?.as_str().map(|s| s.to_string())
                } else if item.get("type")?.as_str()? == "tool-invocation" {
                    let name = item.get("toolName")?.as_str().unwrap_or("unknown");
                    Some(format!("[tool:{}]", name))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

impl Source for PiSource {
    fn name(&self) -> &str {
        "pi"
    }

    fn scan(&self) -> Result<Vec<SourceItemMeta>> {
        if !self.sessions_dir.exists() {
            return Ok(vec![]);
        }
        let mut items = Vec::new();
        for dir_entry in std::fs::read_dir(&self.sessions_dir)? {
            let dir_entry = dir_entry?;
            let path = dir_entry.path();
            if !path.is_dir() {
                continue;
            }
            for file_entry in std::fs::read_dir(&path)? {
                let file_entry = file_entry?;
                let fpath = file_entry.path();
                if fpath.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let meta = std::fs::metadata(&fpath)?;
                let size = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                items.push(SourceItemMeta {
                    item_id: fpath.to_string_lossy().to_string(),
                    fingerprint: format!("{}:{}", size, mtime),
                });
            }
        }
        Ok(items)
    }

    fn load(&self, item_id: &str) -> Result<Vec<ItemChunk>> {
        let path = PathBuf::from(item_id);
        let file = std::fs::File::open(&path)?;
        let reader = BufReader::new(file);

        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut messages: Vec<(String, String, i64)> = Vec::new();
        let mut session_title = String::new();
        let mut project = String::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: PiLine = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let ts = entry
                .timestamp
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or(0);

            match entry.event_type.as_deref() {
                Some("session") => {
                    project = entry.cwd.unwrap_or_default();
                }
                Some("message") => {
                    if let Some(msg) = entry.message {
                        let role = msg.role.unwrap_or_default();
                        let content = msg
                            .content
                            .map(|c| extract_text_content(&c))
                            .unwrap_or_default();
                        if content.is_empty() {
                            continue;
                        }
                        if role == "user" && session_title.is_empty() {
                            session_title = content.chars().take(80).collect();
                        }
                        messages.push((role, content, ts));
                    }
                }
                _ => {} // skip model_change, thinking_level_change, etc.
            }
        }

        // Chunk by user turns
        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut chunk_ts = 0i64;
        let mut ordinal = 0u32;

        for (role, content, ts) in &messages {
            if role == "user" && !current.is_empty() {
                chunks.push(ItemChunk {
                    item_id: item_id.to_string(),
                    chunk_id: format!("pi:{}:{}", stem, ordinal),
                    source: "pi".into(),
                    kind: ItemKind::Session,
                    title: Some(session_title.clone()),
                    timestamp: chunk_ts,
                    ordinal,
                    content: std::mem::take(&mut current),
                    role: Some("user".into()),
                    path: if project.is_empty() {
                        None
                    } else {
                        Some(project.clone())
                    },
                });
                ordinal += 1;
            }
            if chunk_ts == 0 {
                chunk_ts = *ts;
            }
            current.push_str(&format!("{}: {}\n\n", role, content));
        }

        if !current.is_empty() {
            chunks.push(ItemChunk {
                item_id: item_id.to_string(),
                chunk_id: format!("pi:{}:{}", stem, ordinal),
                source: "pi".into(),
                kind: ItemKind::Session,
                title: Some(session_title),
                timestamp: chunk_ts,
                ordinal,
                content: current,
                role: Some("user".into()),
                path: if project.is_empty() {
                    None
                } else {
                    Some(project)
                },
            });
        }

        Ok(chunks)
    }
}
