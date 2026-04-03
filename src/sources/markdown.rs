use anyhow::Result;
use std::path::PathBuf;

use super::{ItemChunk, ItemKind, Source, SourceItemMeta};

pub struct MarkdownSource {
    globs: Vec<String>,
}

impl MarkdownSource {
    pub fn new(globs: Vec<String>) -> Self {
        Self { globs }
    }

    fn resolve_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for pattern in &self.globs {
            let expanded = shellexpand::tilde(pattern).to_string();
            if let Ok(paths) = glob::glob(&expanded) {
                for entry in paths.flatten() {
                    if entry.is_file() {
                        files.push(entry);
                    }
                }
            }
        }
        files
    }

    fn extract_title(content: &str) -> Option<String> {
        content
            .lines()
            .find(|l| l.starts_with("# "))
            .map(|l| l.trim_start_matches("# ").trim().to_string())
    }

    fn extract_date_from_filename(path: &std::path::Path) -> Option<i64> {
        let stem = path.file_stem()?.to_str()?;
        // Match SESJA-YYYY-MM-DD or similar patterns with date
        let re_patterns = ["2026-", "2025-", "2024-"];
        for pat in re_patterns {
            if let Some(pos) = stem.find(pat) {
                let date_str = &stem[pos..];
                if date_str.len() >= 10 {
                    if let Ok(dt) = chrono::NaiveDate::parse_from_str(&date_str[..10], "%Y-%m-%d") {
                        return Some(
                            dt.and_hms_opt(0, 0, 0)?
                                .and_utc()
                                .timestamp_millis(),
                        );
                    }
                }
            }
        }
        None
    }
}

impl Source for MarkdownSource {
    fn name(&self) -> &str {
        "notes"
    }

    fn scan(&self) -> Result<Vec<SourceItemMeta>> {
        let files = self.resolve_files();
        let mut items = Vec::new();
        for path in files {
            let meta = std::fs::metadata(&path)?;
            let size = meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            items.push(SourceItemMeta {
                item_id: path.to_string_lossy().to_string(),
                fingerprint: format!("{}:{}", size, mtime),
            });
        }
        Ok(items)
    }

    fn load(&self, item_id: &str) -> Result<Vec<ItemChunk>> {
        let path = PathBuf::from(item_id);
        let content = std::fs::read_to_string(&path)?;
        let title = Self::extract_title(&content);
        let timestamp = Self::extract_date_from_filename(&path).unwrap_or_else(|| {
            std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        });

        let kind = if path.to_string_lossy().contains("MEMORY") {
            ItemKind::Memory
        } else {
            ItemKind::Note
        };

        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        Ok(vec![ItemChunk {
            item_id: item_id.to_string(),
            chunk_id: format!("notes:{}", stem),
            source: "notes".into(),
            kind,
            title,
            timestamp,
            ordinal: 0,
            content,
            role: None,
            path: Some(item_id.to_string()),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolves_glob_extracts_title_and_date() -> Result<()> {
        let dir = tempdir()?;
        let note_path = dir.path().join("SESJA-2026-04-01.md");
        std::fs::write(&note_path, "# Sprint Log\n\nSome note text.")?;

        let source = MarkdownSource::new(vec![format!("{}/*.md", dir.path().display())]);
        let metas = source.scan()?;
        assert_eq!(metas.len(), 1);

        let chunks = source.load(&metas[0].item_id)?;
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].title.as_deref(), Some("Sprint Log"));
        assert_eq!(chunks[0].kind, ItemKind::Note);

        let expected_ts = chrono::NaiveDate::from_ymd_opt(2026, 4, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(chunks[0].timestamp, expected_ts);
        Ok(())
    }
}
