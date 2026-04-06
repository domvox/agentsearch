use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub hermes: HermesConfig,
    #[serde(default)]
    pub moltis: MoltisConfig,
    #[serde(default)]
    pub nanobot: NanobotConfig,
    #[serde(default)]
    pub notes: NotesConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HermesConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_hermes_path")]
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MoltisConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_moltis_path")]
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NanobotConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_nanobot_path")]
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotesConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_notes_globs")]
    pub globs: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            hermes: HermesConfig::default(),
            moltis: MoltisConfig::default(),
            nanobot: NanobotConfig::default(),
            notes: NotesConfig::default(),
        }
    }
}

impl Default for HermesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: default_hermes_path(),
        }
    }
}

impl Default for MoltisConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: default_moltis_path(),
        }
    }
}

impl Default for NanobotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: default_nanobot_path(),
        }
    }
}

impl Default for NotesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            globs: default_notes_globs(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let path = shellexpand::tilde("~/.config/agentsearch/config.toml").to_string();
        let config_path = std::path::PathBuf::from(path);
        if !config_path.exists() {
            return Ok(Self::default());
        }

        let raw = std::fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read config file {}", config_path.display()))?;
        let parsed: AppConfig = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config file {}", config_path.display()))?;
        Ok(parsed)
    }
}

fn default_true() -> bool {
    true
}

fn default_hermes_path() -> String {
    "~/.hermes/state.db".into()
}

fn default_moltis_path() -> String {
    "~/.moltis/sessions/main.jsonl".into()
}

fn default_nanobot_path() -> String {
    "~/.nanobot/workspace/sessions".into()
}

fn default_notes_globs() -> Vec<String> {
    vec![
        "~/SESSION-*.md".into(),
        "~/INFRA-*.md".into(),
        "~/RESEARCH-*.md".into(),
        "~/CHANGELOG-*.md".into(),
        "~/.hermes/memories/MEMORY.md".into(),
        "~/.hermes/memories/USER.md".into(),
        "~/.nanobot/workspace/memory/MEMORY.md".into(),
        "~/.nanobot/workspace/memory/HISTORY.md".into(),
        "~/.moltis/SOUL.md".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values_are_set() {
        let cfg = AppConfig::default();
        assert!(cfg.hermes.enabled);
        assert!(cfg.moltis.enabled);
        assert!(cfg.nanobot.enabled);
        assert!(cfg.notes.enabled);
        assert!(cfg.hermes.path.contains(".hermes/state.db"));
        assert!(!cfg.notes.globs.is_empty());
    }

    #[test]
    fn loads_from_toml_string() -> Result<()> {
        let raw = r#"
            [hermes]
            enabled = false
            path = "/tmp/hermes.db"

            [moltis]
            path = "/tmp/main.jsonl"

            [notes]
            globs = ["/tmp/*.md"]
        "#;

        let cfg: AppConfig = toml::from_str(raw)?;
        assert!(!cfg.hermes.enabled);
        assert_eq!(cfg.hermes.path, "/tmp/hermes.db");
        assert!(cfg.moltis.enabled);
        assert_eq!(cfg.moltis.path, "/tmp/main.jsonl");
        assert_eq!(cfg.notes.globs, vec!["/tmp/*.md"]);
        Ok(())
    }

    #[test]
    fn disabled_sources_are_respected_in_parsed_config() -> Result<()> {
        let raw = r#"
            [hermes]
            enabled = false
            [moltis]
            enabled = false
            [nanobot]
            enabled = false
            [notes]
            enabled = false
        "#;
        let cfg: AppConfig = toml::from_str(raw)?;
        assert!(!cfg.hermes.enabled);
        assert!(!cfg.moltis.enabled);
        assert!(!cfg.nanobot.enabled);
        assert!(!cfg.notes.enabled);
        Ok(())
    }
}
