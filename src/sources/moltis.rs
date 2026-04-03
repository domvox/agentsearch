use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::{ItemChunk, ItemKind, Source, SourceItemMeta};

pub struct MoltisSource {
    jsonl_path: PathBuf,
    cache: Arc<Mutex<Option<MoltisCache>>>,
}

#[derive(Clone, Default)]
struct MoltisCache {
    runs: HashMap<String, Vec<MoltisMessage>>,
    fingerprints: HashMap<String, String>,
}

#[derive(Clone)]
struct MoltisMessage {
    role: String,
    content: String,
    timestamp: i64,
    seq: u32,
}

impl MoltisSource {
    pub fn new(jsonl_path: PathBuf) -> Self {
        Self {
            jsonl_path,
            cache: Arc::new(Mutex::new(None)),
        }
    }

    fn get_or_build_cache(&self) -> Result<MoltisCache> {
        let mut guard = self.cache.lock().expect("moltis cache lock poisoned");
        if let Some(cache) = &*guard {
            return Ok(cache.clone());
        }

        let file = std::fs::File::open(&self.jsonl_path)?;
        let reader = BufReader::new(file);

        let mut runs: HashMap<String, Vec<MoltisMessage>> = HashMap::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let entry: MoltisEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let Some(run_id) = entry.run_id else {
                continue;
            };

            let role = entry.role.unwrap_or_default();
            let seq = entry.seq.unwrap_or(0);
            let timestamp = entry.created_at.unwrap_or(0);

            let content = match role.as_str() {
                "tool_result" => {
                    let name = entry.tool_name.as_deref().unwrap_or("unknown");
                    let result_str = entry
                        .result
                        .map(|v| {
                            let s = v.to_string();
                            if s.len() > 2048 {
                                let mut end = 2048;
                                while !s.is_char_boundary(end) {
                                    end -= 1;
                                }
                                format!("{}...[truncated]", &s[..end])
                            } else {
                                s
                            }
                        })
                        .unwrap_or_default();
                    format!("[tool:{}] {}", name, result_str)
                }
                _ => entry.content.unwrap_or_default(),
            };

            runs.entry(run_id).or_default().push(MoltisMessage {
                role,
                content,
                timestamp,
                seq,
            });
        }

        let mut fingerprints = HashMap::new();
        for (run_id, msgs) in &mut runs {
            msgs.sort_by_key(|m| (m.seq, m.timestamp));
            fingerprints.insert(run_id.clone(), run_fingerprint(msgs));
        }

        let cache = MoltisCache { runs, fingerprints };
        *guard = Some(cache.clone());
        Ok(cache)
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
        let cache = self.get_or_build_cache()?;

        Ok(cache
            .fingerprints
            .iter()
            .map(|(run_id, fingerprint)| SourceItemMeta {
                item_id: run_id.clone(),
                fingerprint: fingerprint.clone(),
            })
            .collect())
    }

    fn load(&self, item_id: &str) -> Result<Vec<ItemChunk>> {
        let cache = self.get_or_build_cache()?;
        let Some(messages) = cache.runs.get(item_id) else {
            return Ok(vec![]);
        };

        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut chunk_ts = 0i64;
        let mut ordinal = 0u32;

        for msg in messages {
            if msg.role == "user" && !current.is_empty() {
                chunks.push(make_chunk(item_id, ordinal, chunk_ts, &current));
                current.clear();
                ordinal += 1;
                chunk_ts = 0;
            }
            if chunk_ts == 0 {
                chunk_ts = msg.timestamp;
            }
            current.push_str(&format!("{}: {}\n\n", msg.role, msg.content));
        }

        if !current.is_empty() {
            chunks.push(make_chunk(item_id, ordinal, chunk_ts, &current));
        }

        Ok(chunks)
    }
}

fn run_fingerprint(messages: &[MoltisMessage]) -> String {
    let mut hash: u64 = 1469598103934665603;
    for msg in messages {
        for b in msg.role.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
        hash ^= msg.timestamp as u64;
        hash = hash.wrapping_mul(1099511628211);
        hash ^= msg.seq as u64;
        hash = hash.wrapping_mul(1099511628211);
        for b in msg.content.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
    }
    format!("{:x}:{}", hash, messages.len())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn parses_main_jsonl_grouping_and_chunk_boundaries() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("main.jsonl");
        let mut file = std::fs::File::create(&path)?;
        writeln!(
            file,
            "{}",
            serde_json::json!({"run_id":"run-a","role":"assistant","content":"answer 1","created_at":2,"seq":2})
        )?;
        writeln!(
            file,
            "{}",
            serde_json::json!({"run_id":"run-a","role":"user","content":"question 1","created_at":1,"seq":1})
        )?;
        writeln!(
            file,
            "{}",
            serde_json::json!({"run_id":"run-a","role":"user","content":"question 2","created_at":3,"seq":3})
        )?;
        writeln!(
            file,
            "{}",
            serde_json::json!({"run_id":"run-b","role":"user","content":"run b prompt","created_at":10,"seq":1})
        )?;
        writeln!(
            file,
            "{}",
            serde_json::json!({"run_id":"run-b","role":"tool_result","tool_name":"lookup","result":{"ok":true},"created_at":11,"seq":2})
        )?;

        let source = MoltisSource::new(path);
        let metas = source.scan()?;
        assert_eq!(metas.len(), 2);
        assert!(metas.iter().any(|m| m.item_id == "run-a"));
        assert!(metas.iter().any(|m| m.item_id == "run-b"));

        let run_a = source.load("run-a")?;
        assert_eq!(run_a.len(), 2);
        assert!(run_a[0].content.starts_with("user: question 1"));
        assert!(run_a[0].content.contains("assistant: answer 1"));
        assert!(run_a[1].content.contains("user: question 2"));

        let run_b = source.load("run-b")?;
        assert_eq!(run_b.len(), 1);
        assert!(run_b[0].content.contains("[tool:lookup]"));
        Ok(())
    }
}
