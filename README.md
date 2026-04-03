# agentsearch

CLI tool to index and search AI agent sessions across multiple local agents and markdown notes.

Built for developers who run multiple AI coding agents (Hermes, Moltis, Nanobot, etc.) and want to search across all of them in one place.

## Features

- **BM25 full-text search** via [tantivy](https://github.com/quickwit-oss/tantivy) — fast, local, no cloud
- **Chunk-level indexing** — user/assistant turn pairs, not whole sessions or individual messages
- **Incremental indexing** — only re-indexes changed sessions (fingerprint tracking in SQLite)
- **Multiple source adapters:**
  - **Hermes Agent** — reads `~/.hermes/state.db` (SQLite)
  - **Moltis/Zeroclaw** — reads `~/.moltis/sessions/main.jsonl` (grouped by `run_id`)
  - **Nanobot** — reads `~/.nanobot/workspace/sessions/*.jsonl`
  - **Markdown notes** — configurable glob patterns for session notes, memory files, etc.
- **Highlighted snippets** with ANSI colors in terminal
- **JSON output** for piping to `jq`, `fzf`, or other tools
- **Source filtering** — search within a specific agent
- **Tool call indexing** — errors, file paths, and command output from tool calls are searchable (truncated to avoid noise)
- **Memory file support** — indexes `MEMORY.md`, `USER.md`, etc. as searchable documents
- **Single static binary** — zero runtime dependencies

## Install

```bash
cargo install --path .
```

Requires Rust 1.85+.

## Usage

```bash
# Index all detected sources
agentsearch index

# Search across everything
agentsearch search "compaction hermes"

# Filter by source
agentsearch search "KSeF auth" --source hermes

# Limit results
agentsearch search "error" --limit 5

# JSON output (for jq, fzf, scripts)
agentsearch search "auth" --format json

# Minimal output (one line per hit)
agentsearch search "auth" --format minimal

# Show indexed sources and counts
agentsearch sources

# Health check (exit 0 = healthy, exit 1 = no index)
agentsearch health
```

## Config

Optional config file: `~/.config/agentsearch/config.toml`.

```toml
[hermes]
enabled = true
path = "~/.hermes/state.db"

[moltis]
enabled = true
path = "~/.moltis/sessions/main.jsonl"

[nanobot]
enabled = true
path = "~/.nanobot/workspace/sessions"

[notes]
enabled = true
globs = ["~/SESJA-*.md", "~/INFRA-*.md"]
```

Set `enabled = false` to disable a source, or override any path/glob values.

## Source Detection

Sources are auto-detected based on standard paths:

| Source | Path | Format |
|---|---|---|
| Hermes | `~/.hermes/state.db` | SQLite (sessions + messages tables) |
| Moltis | `~/.moltis/sessions/main.jsonl` | JSONL grouped by `run_id` |
| Nanobot | `~/.nanobot/workspace/sessions/*.jsonl` | JSONL per session file |
| Notes | `~/SESJA-*.md`, `~/INFRA-*.md`, etc. | Markdown files |

## How It Works

1. **Sources** parse agent-specific formats into normalized `ItemChunk` structs
2. **Chunks** are user/assistant turn pairs (not individual messages, not whole sessions)
3. **Tantivy** indexes chunks with BM25 for fast full-text search
4. **SQLite sidecar** tracks fingerprints for incremental sync
5. **Search** returns ranked chunks with highlighted snippets, grouped by session

## Data Storage

- Index: `~/.local/share/agentsearch/index/`
- Sync state: `~/.local/share/agentsearch/state.db`

All source data is read-only — agentsearch never modifies your agent files.

## Architecture

```
src/
├── main.rs              # CLI (clap): index, search, sources, health
├── index.rs             # Tantivy schema, indexing, incremental sync, search
└── sources/
    ├── mod.rs           # Source trait, ItemChunk, ItemKind
    ├── hermes.rs        # Hermes Agent (SQLite reader)
    ├── moltis.rs        # Moltis/Zeroclaw (cached JSONL parser)
    ├── nanobot.rs       # Nanobot (per-file JSONL)
    └── markdown.rs      # Markdown notes + memory files
```

## Adding a New Source

Implement the `Source` trait:

```rust
pub trait Source {
    fn name(&self) -> &str;
    fn scan(&self) -> Result<Vec<SourceItemMeta>>;
    fn load(&self, item_id: &str) -> Result<Vec<ItemChunk>>;
}
```

- `scan()` returns lightweight metadata for incremental sync
- `load()` returns chunks for a specific item
- Register your source in `build_sources()` in `main.rs`

## License

MIT
