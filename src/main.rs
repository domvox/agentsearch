mod config;
mod index;
mod sources;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use config::AppConfig;
use index::SearchIndex;
use sources::{
    hermes::HermesSource, markdown::MarkdownSource, moltis::MoltisSource, nanobot::NanobotSource,
    pi::PiSource, Source,
};

#[derive(Parser)]
#[command(
    name = "agentsearch",
    about = "Search across AI agent sessions and notes"
)]
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
        /// Output format
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },
    /// Show indexed sources and counts
    Sources,
    /// Check if index exists and is healthy
    Health,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
    Minimal,
}

fn resolve_path(p: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(p).to_string())
}

fn build_sources(config: &AppConfig) -> Vec<Box<dyn Source>> {
    let mut sources: Vec<Box<dyn Source>> = Vec::new();

    // Hermes
    if config.hermes.enabled {
        let hermes_db = resolve_path(&config.hermes.path);
        if hermes_db.exists() {
            sources.push(Box::new(HermesSource::new(hermes_db)));
        }
    }

    // Moltis
    if config.moltis.enabled {
        let moltis_jsonl = resolve_path(&config.moltis.path);
        if moltis_jsonl.exists() {
            sources.push(Box::new(MoltisSource::new(moltis_jsonl)));
        }
    }

    // Nanobot
    if config.nanobot.enabled {
        let nanobot_dir = resolve_path(&config.nanobot.path);
        if nanobot_dir.exists() {
            sources.push(Box::new(NanobotSource::new(nanobot_dir)));
        }
    }

    // Markdown notes
    if config.notes.enabled {
        sources.push(Box::new(MarkdownSource::new(config.notes.globs.clone())));
    }

    // Pi coding agent
    let pi_dir = resolve_path("~/.pi/agent/sessions");
    if pi_dir.exists() {
        sources.push(Box::new(PiSource::new(pi_dir)));
    }

    sources
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let data_dir = resolve_path(&cli.data_dir);
    std::fs::create_dir_all(&data_dir)?;
    let idx = SearchIndex::new(data_dir.clone());
    let config = AppConfig::load()?;

    match cli.command {
        Commands::Index => {
            let sources = build_sources(&config);
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
        Commands::Search {
            query,
            source,
            limit,
            format,
        } => {
            let hits = idx.search(&query, source.as_deref(), limit)?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&hits)?);
                }
                OutputFormat::Minimal => {
                    for hit in &hits {
                        println!(
                            "{}\t{}\t{}\t{:.2}",
                            hit.source,
                            if hit.title.is_empty() {
                                &hit.item_id
                            } else {
                                &hit.title
                            },
                            hit.path,
                            hit.score,
                        );
                    }
                }
                OutputFormat::Text => {
                    if hits.is_empty() {
                        println!("No results for \"{}\"", query);
                        return Ok(());
                    }
                    for (i, hit) in hits.iter().enumerate() {
                        let ts = chrono::DateTime::from_timestamp_millis(hit.timestamp)
                            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_default();
                        let title = if hit.title.is_empty() {
                            &hit.item_id
                        } else {
                            &hit.title
                        };
                        // Truncate title to 60 chars
                        let title_short: String = title.chars().take(60).collect();

                        // Header line
                        print!("\x1b[1;32m{:>2}.\x1b[0m ", i + 1);
                        print!("\x1b[1m[{}]\x1b[0m ", hit.source);
                        print!("\x1b[36m{}\x1b[0m", title_short);
                        if !hit.path.is_empty() {
                            print!("  \x1b[2m{}\x1b[0m", hit.path);
                        }
                        println!();

                        // Score + date line
                        println!("    \x1b[2m{:.1} pts · {}\x1b[0m", hit.score, ts);

                        // Snippet: clean up, limit to 2 lines
                        let plain = hit
                            .snippet
                            .replace("<b>", "\x1b[1;33m")
                            .replace("</b>", "\x1b[0m")
                            .replace("&amp;", "&")
                            .replace("&lt;", "<")
                            .replace("&gt;", ">")
                            .replace("&quot;", "\"")
                            .replace("\\n", " ");
                        // Take first 200 chars, clean whitespace
                        let snippet_clean: String = plain
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        let snippet_short: String = if snippet_clean.len() > 200 {
                            let mut end = 200;
                            while !snippet_clean.is_char_boundary(end) { end -= 1; }
                            format!("{}…", &snippet_clean[..end])
                        } else {
                            snippet_clean
                        };
                        println!("    {}", snippet_short);
                        println!();
                    }
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
