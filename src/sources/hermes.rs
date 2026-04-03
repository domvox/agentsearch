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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_test_db(path: &std::path::Path) -> Result<()> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT,
                message_count INTEGER,
                started_at REAL,
                ended_at REAL
            );
            CREATE TABLE messages (
                session_id TEXT,
                role TEXT,
                content TEXT,
                timestamp REAL,
                tool_name TEXT
            );",
        )?;

        conn.execute(
            "INSERT INTO sessions (id, title, message_count, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["s1", "Session One", 5i64, 1.0f64, 5.0f64],
        )?;
        conn.execute(
            "INSERT INTO sessions (id, title, message_count, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["s2", "Session Two", 1i64, 10.0f64, 11.0f64],
        )?;

        let messages = vec![
            ("s1", "user", "hello", 1.0, Option::<String>::None),
            ("s1", "assistant", "hi there", 2.0, Option::<String>::None),
            ("s1", "user", "use tool", 3.0, Option::<String>::None),
            ("s1", "assistant", "running", 4.0, Option::<String>::None),
            ("s1", "tool", "{\"ok\":true}", 5.0, Some("fetch".to_string())),
            ("s2", "user", "another session", 10.0, Option::<String>::None),
        ];

        for (sid, role, content, ts, tool_name) in messages {
            conn.execute(
                "INSERT INTO messages (session_id, role, content, timestamp, tool_name)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![sid, role, content, ts, tool_name],
            )?;
        }
        Ok(())
    }

    #[test]
    fn scan_and_load_from_sqlite() -> Result<()> {
        let dir = tempdir()?;
        let db_path = dir.path().join("state.db");
        create_test_db(&db_path)?;

        let source = HermesSource::new(db_path);
        let mut metas = source.scan()?;
        metas.sort_by(|a, b| a.item_id.cmp(&b.item_id));

        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].item_id, "s1");
        assert_eq!(metas[0].fingerprint, "5:5");
        assert_eq!(metas[1].item_id, "s2");

        let chunks = source.load("s1")?;
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].title.as_deref(), Some("Session One"));
        assert!(chunks[0].content.contains("user: hello"));
        assert!(chunks[0].content.contains("assistant: hi there"));
        assert!(chunks[1].content.contains("user: use tool"));
        assert!(chunks[1].content.contains("[tool:fetch] {\"ok\":true}"));
        Ok(())
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
