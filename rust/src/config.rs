use anyhow::{ensure, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
#[derive(Clone, Deserialize)]
pub struct Config {
    pub apple_app_token: String,
    pub apple_user_token: String,
    pub agent_token: String,
    #[serde(default = "ffmpeg")]
    pub ffmpeg: String,
    #[serde(default = "aplay")]
    pub aplay: String,
    #[serde(default = "pcm")]
    pub pcm: String,
    #[serde(default = "volume_limit")]
    pub audio_volume_limit: f64,
    #[serde(default = "native_volume_file")]
    pub native_volume_file: PathBuf,
    #[serde(default = "native_volume_max")]
    pub native_volume_max: f64,
    #[serde(default = "port")]
    pub port: u16,
    #[serde(default = "ipc_port")]
    pub recognition_port: u16,
    #[serde(default = "download_limit")]
    pub max_download_bytes: u64,
    #[serde(default)]
    pub airplay_bus: Option<String>,
    #[serde(default)]
    pub gemini: GeminiOptions,
}
#[derive(Clone, Default, Deserialize)]
pub struct GeminiOptions {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub alice_device_names: Vec<String>,
}
fn native_volume_file() -> PathBuf {
    "/data/quasar/data/last_vendor_volume".into()
}
fn native_volume_max() -> f64 {
    20.
}
fn aplay() -> String {
    "/usr/bin/aplay".into()
}
fn pcm() -> String {
    "plug:dmix_spk".into()
}
fn volume_limit() -> f64 {
    1.
}
fn ffmpeg() -> String {
    "/opt/bin/ffmpeg".into()
}
fn port() -> u16 {
    8099
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
        ensure!(
            c.audio_volume_limit.is_finite() && (0.0..=1.0).contains(&c.audio_volume_limit),
            "Invalid audio volume limit"
        );
        ensure!(
            c.native_volume_max.is_finite() && c.native_volume_max > 0.0,
            "Invalid native volume scale"
        );
        ensure!(!c.agent_token.is_empty(), "Missing API token");
        ensure!(
            !c.apple_app_token.starts_with("YOUR_"),
            "Apple Music profile is not configured"
        );
        Ok(c)
    }
}
