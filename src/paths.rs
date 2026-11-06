//! Config and runtime file locations.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Persisted settings: which device to talk to and the channel that worked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub address: String,
    /// Last RFCOMM channel that answered the status request.
    pub channel: Option<u8>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            address: "E4:EC:E8:43:CD:07".into(),
            channel: None,
        }
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/root".into()))
}

pub fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".config"))
        .join("galaxybuds")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.json")
}

pub fn runtime_dir() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("galaxybuds")
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("ipc.sock")
}

pub fn state_path() -> PathBuf {
    runtime_dir().join("state.json")
}

impl Config {
    pub fn load() -> Config {
        std::fs::read_to_string(config_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir())?;
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(config_file(), json)
    }
}
