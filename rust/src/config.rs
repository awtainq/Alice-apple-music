use anyhow::{ensure, Result};
use serde::Deserialize;
use std::path::Path;
#[derive(Clone, Deserialize)]
pub struct Config {
    pub apple_app_token: String,
    pub apple_user_token: String,
    pub yandex_music_token: String,
    pub yandex_x_token: String,
    pub station_device_id: String,
    pub station_platform: String,
    pub agent_token: String,
    #[serde(default = "ffmpeg")]
    pub ffmpeg: String,
    #[serde(default = "port")]
    pub port: u16,
    #[serde(default = "station_port")]
    pub station_port: u16,
    #[serde(default = "ipc_port")]
    pub recognition_port: u16,
    #[serde(default = "download_limit")]
    pub max_download_bytes: u64,
    #[serde(default)]
    pub airplay_bus: Option<String>,
}
fn ffmpeg() -> String {
    "/opt/bin/ffmpeg".into()
}
fn port() -> u16 {
    8099
}
fn station_port() -> u16 {
    1961
}
fn ipc_port() -> u16 {
    9875
}
fn download_limit() -> u64 {
    32 * 1024 * 1024
}
impl Config {
    pub fn load(private: &Path) -> Result<Self> {
        let c: Self = serde_json::from_slice(&std::fs::read(private.join("config.json"))?)?;
        ensure!(!c.agent_token.is_empty(), "Missing API token");
        ensure!(
            !c.apple_app_token.starts_with("YOUR_"),
            "Apple Music profile is not configured"
        );
        Ok(c)
    }
}
