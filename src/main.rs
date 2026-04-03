mod index;
mod sources;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use index::SearchIndex;
use sources::{hermes::HermesSource, markdown::MarkdownSource, moltis::MoltisSource, nanobot::NanobotSource, Source};

#[derive(Parser)]
#[command(name = "agentsearch", about = "Search across AI agent sessions and notes")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Data directory for index and state
    #[arg(long, default_value = "~/.local/share/agentsearch")]
    data_dir: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Index all configured sources
    Index,
    /// Search indexed sessions and notes
    Search {
        query: String,
        /// Filter by source name
        #[arg(long)]
        source: Option<String>,
        /// Max results
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show indexed sources and counts
    Sources,
    /// Check if index exists and is healthy
    Health,
}

fn resolve_path(p: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(p).to_string())
}

fn build_sources() -> Vec<Box<dyn Source>> {
    let mut sources: Vec<Box<dyn Source>> = Vec::new();

    // Hermes
    let hermes_db = resolve_path("~/.hermes/state.db");
    if hermes_db.exists() {
        sources.push(Box::new(HermesSource::new(hermes_db)));
    }

    // Moltis
    let moltis_jsonl = resolve_path("~/.moltis/sessions/main.jsonl");
    if moltis_jsonl.exists() {
        sources.push(Box::new(MoltisSource::new(moltis_jsonl)));
    }

    // Nanobot
    let nanobot_dir = resolve_path("~/.nanobot/workspace/sessions");
    if nanobot_dir.exists() {
        sources.push(Box::new(NanobotSource::new(nanobot_dir)));
    }

    // Markdown notes
    let md_globs = vec![
        "~/SESJA-*.md".into(),
        "~/INFRA-*.md".into(),
        "~/RESEARCH-*.md".into(),
        "~/CHANGELOG-*.md".into(),
        "~/.hermes/memories/MEMORY.md".into(),
        "~/.hermes/memories/USER.md".into(),
        "~/.nanobot/workspace/memory/MEMORY.md".into(),
        "~/.nanobot/workspace/memory/HISTORY.md".into(),
        "~/.moltis/SOUL.md".into(),
    ];
    sources.push(Box::new(MarkdownSource::new(md_globs)));

    sources
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let data_dir = resolve_path(&cli.data_dir);
    std::fs::create_dir_all(&data_dir)?;
    let idx = SearchIndex::new(data_dir.clone());

    match cli.command {
        Commands::Index => {
            let sources = build_sources();
            println!("Indexing {} source(s)...", sources.len());
            for s in &sources {
                println!("  - {}", s.name());
            }
            let stats = idx.index_sources(&sources)?;
            println!(
                "Done: {} indexed, {} skipped, {} removed, {} chunks, {} errors",
                stats.indexed, stats.skipped, stats.removed, stats.chunks, stats.errors
            );
        }
        Commands::Search { query, source, limit, json } => {
            let hits = idx.search(&query, source.as_deref(), limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else {
                if hits.is_empty() {
                    println!("No results for \"{}\"", query);
                    return Ok(());
                }
                for hit in &hits {
                    let ts = chrono::DateTime::from_timestamp_millis(hit.timestamp)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default();
                    println!(
                        "\x1b[1m[{}]\x1b[0m \x1b[36m{}\x1b[0m {} \x1b[2m(score: {:.2}, {})\x1b[0m",
                        hit.source,
                        if hit.title.is_empty() { &hit.item_id } else { &hit.title },
                        if !hit.path.is_empty() { format!("\x1b[2m{}\x1b[0m", hit.path) } else { String::new() },
                        hit.score,
                        ts,
                    );
                    // Strip HTML tags from snippet for terminal display
                    let plain = hit.snippet
                        .replace("<b>", "\x1b[1;33m")
                        .replace("</b>", "\x1b[0m");
                    println!("  {}\n", plain);
                }
            }
        }
        Commands::Sources => {
            let stats = idx.source_stats()?;
            if stats.is_empty() {
                println!("No sources indexed. Run `agentsearch index` first.");
            } else {
                println!("{:<15} {}", "SOURCE", "ITEMS");
                for (source, count) in &stats {
                    println!("{:<15} {}", source, count);
                }
            }
        }
        Commands::Health => {
            let index_path = data_dir.join("index").join("meta.json");
            if index_path.exists() {
                println!("OK: index exists");
                std::process::exit(0);
            } else {
                println!("UNHEALTHY: no index. Run `agentsearch index`");
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
