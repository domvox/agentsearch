use anyhow::Result;
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use super::{ItemChunk, ItemKind, Source, SourceItemMeta};

/// Generic JSONL source for Nanobot sessions.
/// Each file is one session. First line may be metadata (_type: "metadata").
/// Subsequent lines: {role, content, timestamp, tool_calls?}
pub struct NanobotSource {
    sessions_dir: PathBuf,
}

impl NanobotSource {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }
}

#[derive(Deserialize)]
struct NanobotLine {
    #[serde(default)]
    _type: Option<String>,
    role: Option<String>,
    content: Option<String>,
    timestamp: Option<String>,
    tool_calls: Option<serde_json::Value>,
}

impl Source for NanobotSource {
    fn name(&self) -> &str {
        "nanobot"
    }

    fn scan(&self) -> Result<Vec<SourceItemMeta>> {
        if !self.sessions_dir.exists() {
            return Ok(vec![]);
        }
        let mut items = Vec::new();
        for entry in std::fs::read_dir(&self.sessions_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") { continue; }
            let meta = std::fs::metadata(&path)?;
            let size = meta.len();
            let mtime = meta.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()).unwrap_or(0);
            items.push(SourceItemMeta {
                item_id: path.to_string_lossy().to_string(),
                fingerprint: format!("{}:{}", size, mtime),
            });
        }
        Ok(items)
    }

    fn load(&self, item_id: &str) -> Result<Vec<ItemChunk>> {
        let path = PathBuf::from(item_id);
        let file = std::fs::File::open(&path)?;
        let reader = BufReader::new(file);

        let stem = path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut messages: Vec<(String, String, i64)> = Vec::new();
        let mut session_title = stem.clone();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() { continue; }
            let entry: NanobotLine = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Skip metadata lines
            if entry._type.as_deref() == Some("metadata") { continue; }

            let role = entry.role.unwrap_or_default();
            let content = entry.content.unwrap_or_default();
            if content.is_empty() && entry.tool_calls.is_none() { continue; }

            let ts = entry.timestamp.as_deref()
                .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok())
                .map(|dt| dt.and_utc().timestamp_millis())
                .unwrap_or(0);

            // For assistant with tool_calls but no content, format tool call info
            let text = if content.is_empty() {
                if let Some(tools) = &entry.tool_calls {
                    format_tool_calls(tools)
                } else {
                    continue;
                }
            } else {
                content
            };

            // Use first user message as title
            if role == "user" && messages.is_empty() {
                session_title = text.chars().take(80).collect();
            }

            messages.push((role, text, ts));
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
                    chunk_id: format!("nanobot:{}:{}", stem, ordinal),
                    source: "nanobot".into(),
                    kind: ItemKind::Session,
                    title: Some(session_title.clone()),
                    timestamp: chunk_ts,
                    ordinal,
                    content: std::mem::take(&mut current),
                    role: Some("user".into()),
                    path: Some(item_id.to_string()),
                });
                ordinal += 1;
            }
            if chunk_ts == 0 { chunk_ts = *ts; }
            current.push_str(&format!("{}: {}\n\n", role, content));
        }

        if !current.is_empty() {
            chunks.push(ItemChunk {
                item_id: item_id.to_string(),
                chunk_id: format!("nanobot:{}:{}", stem, ordinal),
                source: "nanobot".into(),
                kind: ItemKind::Session,
                title: Some(session_title),
                timestamp: chunk_ts,
                ordinal,
                content: current,
                role: Some("user".into()),
                path: Some(item_id.to_string()),
            });
        }

        Ok(chunks)
    }
}

fn format_tool_calls(tools: &serde_json::Value) -> String {
    if let Some(arr) = tools.as_array() {
        arr.iter().filter_map(|t| {
            let name = t.get("function")?.get("name")?.as_str()?;
            let args = t.get("function")?.get("arguments")?.as_str().unwrap_or("");
            let truncated = if args.len() > 512 {
                let mut end = 512;
                while !args.is_char_boundary(end) { end -= 1; }
                format!("{}...", &args[..end])
            } else {
                args.to_string()
            };
            Some(format!("[tool:{}] {}", name, truncated))
        }).collect::<Vec<_>>().join("\n")
    } else {
        String::new()
    }
}
