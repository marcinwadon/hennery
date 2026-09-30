//! `<data>/config.toml` (kernel spec §1, §2): the collector's settings that
//! are not the operator's to edit in the UI. Precedence, per setting:
//! command-line flags, then the environment (`HENNERY_*`), then this file,
//! then the defaults. Flags and the environment are clap's: it takes a
//! flag over its variable. This file fills in what neither gave.
//!
//! ```toml
//! listen = ["127.0.0.1:7117", "100.64.0.7:7117"]
//! public_url = "https://hennery.example"
//! ```
//!
//! An unknown key is refused, so a typo fails the start instead of being
//! ignored. No secret belongs here: the file is read, never checked for
//! its mode.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// The file's name in the collector's data directory.
pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    /// The addresses to listen on (kernel spec §7).
    pub listen: Option<Vec<String>>,
    /// Where browsers reach the collector, until setup stores its own
    /// (kernel spec §3.1): the setup link names it.
    pub public_url: Option<String>,
}

impl FileConfig {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// `dir/config.toml`, or the defaults when there is none. Reading it
    /// creates nothing.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).with_context(|| format!("{}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("read {}", path.display())),
        }
    }

    /// The listen addresses: `given` (a flag or `HENNERY_LISTEN`) if any,
    /// else the file's. Empty means the default.
    pub fn listen(&self, given: &[String]) -> Vec<String> {
        if given.is_empty() {
            self.listen.clone().unwrap_or_default()
        } else {
            given.to_vec()
        }
    }

    /// `public_url`: `given` (a flag or `HENNERY_PUBLIC_URL`) if any, else
    /// the file's.
    pub fn public_url(&self, given: Option<&str>) -> Option<String> {
        given.map(str::to_string).or_else(|| self.public_url.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_holds_listen_and_public_url_and_nothing_else() {
        let config = FileConfig::parse(
            "listen = [\"127.0.0.1:7117\", \"[::1]:7117\"]\npublic_url = \"https://hennery.example\"\n",
        )
        .unwrap();
        assert_eq!(
            config,
            FileConfig {
                listen: Some(vec!["127.0.0.1:7117".into(), "[::1]:7117".into()]),
                public_url: Some("https://hennery.example".into()),
            }
        );
        assert_eq!(FileConfig::parse("").unwrap(), FileConfig::default());
        let typo = FileConfig::parse("listens = [\"127.0.0.1:7117\"]\n").unwrap_err();
        assert!(format!("{typo:#}").contains("unknown field"), "{typo:#}");
        assert!(FileConfig::parse("listen = \"127.0.0.1:7117\"\n").is_err());
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let dir = std::env::temp_dir().join(format!("hennery-config-missing-{}", std::process::id()));
        assert_eq!(FileConfig::load(&dir).unwrap(), FileConfig::default());
        assert!(!dir.exists(), "loading made the directory");
    }

    #[test]
    fn flags_and_the_environment_win_over_the_file() {
        let file = FileConfig {
            listen: Some(vec!["127.0.0.1:1".into()]),
            public_url: Some("https://file.example".into()),
        };
        assert_eq!(file.listen(&[]), ["127.0.0.1:1"]);
        assert_eq!(file.listen(&["127.0.0.1:2".into()]), ["127.0.0.1:2"]);
        assert_eq!(file.public_url(None).as_deref(), Some("https://file.example"));
        assert_eq!(
            file.public_url(Some("https://flag.example")).as_deref(),
            Some("https://flag.example")
        );
        let empty = FileConfig::default();
        assert!(empty.listen(&[]).is_empty());
        assert_eq!(empty.public_url(None), None);
    }
}
