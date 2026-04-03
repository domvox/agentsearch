use anyhow::Result;
use serde::{Deserialize, Serialize};

/// What kind of indexed item this is
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ItemKind {
    Session,
    Note,
    Memory,
}

impl std::fmt::Display for ItemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session => write!(f, "session"),
            Self::Note => write!(f, "note"),
            Self::Memory => write!(f, "memory"),
        }
    }
}

/// A chunk is the unit of indexing — one or more messages grouped together
#[derive(Debug, Clone)]
pub struct ItemChunk {
    pub item_id: String,
    pub chunk_id: String,
    pub source: String,
    pub kind: ItemKind,
    pub title: Option<String>,
    pub timestamp: i64, // unix ms
    pub ordinal: u32,
    pub content: String,
    pub role: Option<String>,
    pub path: Option<String>,
}

/// Metadata for incremental sync — cheap to compute, avoids re-parsing
#[derive(Debug, Clone)]
pub struct SourceItemMeta {
    pub item_id: String,
    pub fingerprint: String, // source-specific: message_count, mtime+size, etc.
}

pub trait Source {
    fn name(&self) -> &str;
    fn scan(&self) -> Result<Vec<SourceItemMeta>>;
    fn load(&self, item_id: &str) -> Result<Vec<ItemChunk>>;
}

pub mod hermes;
pub mod markdown;
pub mod moltis;
pub mod nanobot;
