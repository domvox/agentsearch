use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::PathBuf;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::snippet::SnippetGenerator;
use tantivy::{doc, Index, IndexWriter, ReloadPolicy, TantivyDocument};

use crate::sources::Source;

pub struct SearchIndex {
    data_dir: PathBuf,
}

fn build_schema() -> Schema {
    let mut builder = Schema::builder();
    let text_opts = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default().set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();
    builder.add_text_field("item_id", STRING | STORED);
    builder.add_text_field("chunk_id", STRING | STORED);
    builder.add_text_field("source", STRING | STORED);
    builder.add_text_field("kind", STRING | STORED);
    builder.add_text_field("title", text_opts.clone());
    builder.add_text_field("content", text_opts);
    builder.add_i64_field("timestamp", INDEXED | STORED | FAST);
    builder.add_u64_field("ordinal", STORED | FAST);
    builder.add_text_field("path", STORED);
    builder.add_text_field("role", STORED);
    builder.build()
}

impl SearchIndex {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    fn index_path(&self) -> PathBuf {
        self.data_dir.join("index")
    }

    fn state_path(&self) -> PathBuf {
        self.data_dir.join("state.db")
    }

    fn open_state(&self) -> Result<Connection> {
        let conn = Connection::open(self.state_path())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sync_state (
                source TEXT NOT NULL,
                item_id TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                PRIMARY KEY (source, item_id)
            )",
        )?;
        Ok(conn)
    }

    fn get_or_create_index(&self) -> Result<Index> {
        let path = self.index_path();
        std::fs::create_dir_all(&path)?;
        let schema = build_schema();
        let index = if path.join("meta.json").exists() {
            Index::open_in_dir(&path).unwrap_or_else(|_| {
                std::fs::remove_dir_all(&path).ok();
                std::fs::create_dir_all(&path).unwrap();
                Index::create_in_dir(&path, schema.clone()).unwrap()
            })
        } else {
            Index::create_in_dir(&path, schema)?
        };
        Ok(index)
    }

    pub fn index_sources(&self, sources: &[Box<dyn Source>]) -> Result<IndexStats> {
        let index = self.get_or_create_index()?;
        let schema = index.schema();
        let state = self.open_state()?;
        let mut writer: IndexWriter = index.writer(50_000_000)?;
        let mut stats = IndexStats::default();

        let f_item_id = schema.get_field("item_id").unwrap();
        let f_chunk_id = schema.get_field("chunk_id").unwrap();
        let f_source = schema.get_field("source").unwrap();
        let f_kind = schema.get_field("kind").unwrap();
        let f_title = schema.get_field("title").unwrap();
        let f_content = schema.get_field("content").unwrap();
        let f_timestamp = schema.get_field("timestamp").unwrap();
        let f_ordinal = schema.get_field("ordinal").unwrap();
        let f_path = schema.get_field("path").unwrap();
        let f_role = schema.get_field("role").unwrap();

        for source in sources {
            let source_name = source.name();
            match source.scan() {
                Ok(metas) => {
                    let existing = self.load_fingerprints(&state, source_name)?;
                    let mut seen_ids: Vec<String> = Vec::new();

                    for meta in &metas {
                        seen_ids.push(meta.item_id.clone());
                        if existing.get(&meta.item_id) == Some(&meta.fingerprint) {
                            stats.skipped += 1;
                            continue;
                        }

                        // Delete old chunks
                        writer
                            .delete_term(tantivy::Term::from_field_text(f_item_id, &meta.item_id));

                        match source.load(&meta.item_id) {
                            Ok(chunks) => {
                                for chunk in &chunks {
                                    writer.add_document(doc!(
                                        f_item_id => chunk.item_id.as_str(),
                                        f_chunk_id => chunk.chunk_id.as_str(),
                                        f_source => chunk.source.as_str(),
                                        f_kind => chunk.kind.to_string(),
                                        f_title => chunk.title.as_deref().unwrap_or(""),
                                        f_content => chunk.content.as_str(),
                                        f_timestamp => chunk.timestamp,
                                        f_ordinal => chunk.ordinal as u64,
                                        f_path => chunk.path.as_deref().unwrap_or(""),
                                        f_role => chunk.role.as_deref().unwrap_or("")
                                    ))?;
                                    stats.chunks += 1;
                                }
                                stats.indexed += 1;
                                self.save_fingerprint(
                                    &state,
                                    source_name,
                                    &meta.item_id,
                                    &meta.fingerprint,
                                )?;
                            }
                            Err(e) => {
                                eprintln!("  WARN: {}/{}: {}", source_name, meta.item_id, e);
                                stats.errors += 1;
                            }
                        }
                    }

                    // Remove deleted items
                    for (old_id, _) in &existing {
                        if !seen_ids.contains(old_id) {
                            writer.delete_term(tantivy::Term::from_field_text(f_item_id, old_id));
                            state.execute(
                                "DELETE FROM sync_state WHERE source = ?1 AND item_id = ?2",
                                rusqlite::params![source_name, old_id],
                            )?;
                            stats.removed += 1;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("  WARN: scan {} failed: {}", source_name, e);
                    stats.errors += 1;
                }
            }
        }

        writer.commit()?;
        Ok(stats)
    }

    fn load_fingerprints(
        &self,
        conn: &Connection,
        source: &str,
    ) -> Result<HashMap<String, String>> {
        let mut stmt =
            conn.prepare("SELECT item_id, fingerprint FROM sync_state WHERE source = ?1")?;
        let map = stmt
            .query_map([source], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(map)
    }

    fn save_fingerprint(
        &self,
        conn: &Connection,
        source: &str,
        item_id: &str,
        fingerprint: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO sync_state (source, item_id, fingerprint) VALUES (?1, ?2, ?3)",
            rusqlite::params![source, item_id, fingerprint],
        )?;
        Ok(())
    }

    pub fn search(
        &self,
        query: &str,
        source_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let index = Index::open_in_dir(self.index_path())
            .context("Index not found. Run `agentsearch index` first.")?;
        let schema = index.schema();
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        let searcher = reader.searcher();

        let f_title = schema.get_field("title").unwrap();
        let f_content = schema.get_field("content").unwrap();
        let f_source = schema.get_field("source").unwrap();
        let f_item_id = schema.get_field("item_id").unwrap();
        let f_chunk_id = schema.get_field("chunk_id").unwrap();
        let f_kind = schema.get_field("kind").unwrap();
        let f_timestamp = schema.get_field("timestamp").unwrap();
        let f_path = schema.get_field("path").unwrap();

        let query_parser = QueryParser::for_index(&index, vec![f_title, f_content]);
        let parsed = query_parser.parse_query(query)?;

        let top_docs =
            searcher.search(&parsed, &TopDocs::with_limit(limit * 3).order_by_score())?;

        let mut snippet_gen = SnippetGenerator::create(&searcher, &*parsed, f_content)?;
        snippet_gen.set_max_num_chars(300);

        let mut hits: Vec<SearchHit> = Vec::new();

        for (score, doc_addr) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_addr)?;

            let source_val = doc
                .get_first(f_source)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(filter) = source_filter {
                if source_val != filter {
                    continue;
                }
            }

            let snippet = snippet_gen.snippet_from_doc(&doc);
            let snippet_html = snippet.to_html();
            let snippet_text = if snippet_html.is_empty() {
                doc.get_first(f_content)
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        if s.len() > 200 {
                            format!("{}...", &s[..200])
                        } else {
                            s.to_string()
                        }
                    })
                    .unwrap_or_default()
            } else {
                snippet_html
            };

            hits.push(SearchHit {
                item_id: doc
                    .get_first(f_item_id)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
                chunk_id: doc
                    .get_first(f_chunk_id)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
                source: source_val.into(),
                kind: doc
                    .get_first(f_kind)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
                title: doc
                    .get_first(f_title)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
                snippet: snippet_text,
                score,
                timestamp: doc
                    .get_first(f_timestamp)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
                path: doc
                    .get_first(f_path)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
            });

            if hits.len() >= limit {
                break;
            }
        }

        Ok(hits)
    }

    pub fn source_stats(&self) -> Result<Vec<(String, i64)>> {
        let state = self.open_state()?;
        let mut stmt = state.prepare("SELECT source, COUNT(*) FROM sync_state GROUP BY source")?;
        let stats = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(stats)
    }
}

#[derive(Debug, Default)]
pub struct IndexStats {
    pub indexed: usize,
    pub skipped: usize,
    pub removed: usize,
    pub chunks: usize,
    pub errors: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct SearchHit {
    pub item_id: String,
    pub chunk_id: String,
    pub source: String,
    pub kind: String,
    pub title: String,
    pub snippet: String,
    pub score: f32,
    pub timestamp: i64,
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{ItemChunk, ItemKind, Source, SourceItemMeta};
    use tempfile::tempdir;

    struct TestSource {
        name: String,
        metas: Vec<SourceItemMeta>,
        chunks: HashMap<String, Vec<ItemChunk>>,
    }

    impl Source for TestSource {
        fn name(&self) -> &str {
            &self.name
        }

        fn scan(&self) -> Result<Vec<SourceItemMeta>> {
            Ok(self.metas.clone())
        }

        fn load(&self, item_id: &str) -> Result<Vec<ItemChunk>> {
            Ok(self.chunks.get(item_id).cloned().unwrap_or_default())
        }
    }

    #[test]
    fn full_index_and_search_cycle() -> Result<()> {
        let dir = tempdir()?;
        let idx = SearchIndex::new(dir.path().to_path_buf());

        let item_id = "item-1".to_string();
        let chunks = vec![
            ItemChunk {
                item_id: item_id.clone(),
                chunk_id: "c1".into(),
                source: "test".into(),
                kind: ItemKind::Session,
                title: Some("Rust indexing".into()),
                timestamp: 1_711_929_600_000,
                ordinal: 0,
                content: "user: tantivy search rust snippet".into(),
                role: Some("user".into()),
                path: Some("/tmp/a".into()),
            },
            ItemChunk {
                item_id: item_id.clone(),
                chunk_id: "c2".into(),
                source: "test".into(),
                kind: ItemKind::Session,
                title: Some("Other".into()),
                timestamp: 1_711_929_700_000,
                ordinal: 1,
                content: "assistant: unrelated text".into(),
                role: Some("assistant".into()),
                path: Some("/tmp/b".into()),
            },
        ];
        let source = TestSource {
            name: "test".into(),
            metas: vec![SourceItemMeta {
                item_id: item_id.clone(),
                fingerprint: "fp-1".into(),
            }],
            chunks: HashMap::from([(item_id.clone(), chunks)]),
        };

        let boxed: Vec<Box<dyn Source>> = vec![Box::new(source)];
        let stats = idx.index_sources(&boxed)?;
        assert_eq!(stats.indexed, 1);
        assert_eq!(stats.chunks, 2);

        let hits = idx.search("tantivy", Some("test"), 5)?;
        assert!(!hits.is_empty());
        assert_eq!(hits[0].source, "test");
        assert!(hits[0].score > 0.0);
        assert!(hits[0].snippet.contains("tantivy") || hits[0].snippet.contains("<b>"));
        Ok(())
    }
}
