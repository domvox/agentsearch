use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::PathBuf;

use super::{ItemChunk, ItemKind, Source, SourceItemMeta};

pub struct HermesSource {
    db_path: PathBuf,
}

impl HermesSource {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn open_db(&self) -> Result<Connection> {
        let conn = Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("Failed to open Hermes DB: {:?}", self.db_path))?;
        Ok(conn)
    }
}

impl Source for HermesSource {
    fn name(&self) -> &str {
        "hermes"
    }

    fn scan(&self) -> Result<Vec<SourceItemMeta>> {
        let conn = self.open_db()?;
        let mut stmt = conn.prepare(
            "SELECT id, message_count, ended_at FROM sessions ORDER BY started_at DESC",
        )?;
        let items = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let msg_count: i64 = row.get(1)?;
                let ended_at: Option<f64> = row.get(2)?;
                Ok(SourceItemMeta {
                    item_id: id,
                    fingerprint: format!("{}:{}", msg_count, ended_at.unwrap_or(0.0) as i64),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    fn load(&self, item_id: &str) -> Result<Vec<ItemChunk>> {
        let conn = self.open_db()?;

        // Get session title
        let title: Option<String> = conn
            .query_row(
                "SELECT title FROM sessions WHERE id = ?1",
                [item_id],
                |row| row.get(0),
            )
            .ok();

        // Load messages, chunk by user/assistant turn pairs
        let mut stmt = conn.prepare(
            "SELECT role, content, timestamp, tool_name FROM messages \
             WHERE session_id = ?1 ORDER BY timestamp ASC",
        )?;

        let messages: Vec<(String, String, f64, Option<String>)> = stmt
            .query_map([item_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1).unwrap_or_default(),
                    row.get::<_, f64>(2).unwrap_or(0.0),
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut chunks = Vec::new();
        let mut current_chunk = String::new();
        let mut chunk_ts = 0i64;
        let mut ordinal = 0u32;
        let mut chunk_role = None;

        for (role, content, ts, tool_name) in &messages {
            let ts_ms = (*ts * 1000.0) as i64;

            // Start new chunk on each user message (except first)
            if role == "user" && !current_chunk.is_empty() {
                chunks.push(ItemChunk {
                    item_id: item_id.to_string(),
                    chunk_id: format!("{}:{}", item_id, ordinal),
                    source: "hermes".into(),
                    kind: ItemKind::Session,
                    title: title.clone(),
                    timestamp: chunk_ts,
                    ordinal,
                    content: std::mem::take(&mut current_chunk),
                    role: chunk_role.take(),
                    path: None,
                });
                ordinal += 1;
            }

            if chunk_ts == 0 {
                chunk_ts = ts_ms;
            }

            match role.as_str() {
                "user" => {
                    chunk_role = Some("user".into());
                    current_chunk.push_str(&format!("user: {}\n\n", content));
                }
                "assistant" => {
                    current_chunk.push_str(&format!("assistant: {}\n\n", content));
                }
                "tool" => {
                    let name = tool_name.as_deref().unwrap_or("unknown");
                    let truncated = if content.len() > 2048 {
                        let mut end = 2048;
                        while !content.is_char_boundary(end) { end -= 1; }
                        format!("{}...[truncated {}B]", &content[..end], content.len())
                    } else {
                        content.clone()
                    };
                    current_chunk.push_str(&format!("[tool:{}] {}\n\n", name, truncated));
                }
                _ => {
                    current_chunk.push_str(&format!("{}: {}\n\n", role, content));
                }
            }
        }

        // Flush last chunk
        if !current_chunk.is_empty() {
            chunks.push(ItemChunk {
                item_id: item_id.to_string(),
                chunk_id: format!("{}:{}", item_id, ordinal),
                source: "hermes".into(),
                kind: ItemKind::Session,
                title: title.clone(),
                timestamp: chunk_ts,
                ordinal,
                content: current_chunk,
                role: chunk_role,
                path: None,
            });
        }

        Ok(chunks)
    }
}
