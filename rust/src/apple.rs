use crate::config::Config;
use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use futures_util::StreamExt;
use reqwest::{Client, Method, Response};
use rsa::{pkcs1::DecodeRsaPrivateKey, pkcs8::DecodePrivateKey, RsaPrivateKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};
use tokio::{io::AsyncWriteExt, sync::Mutex};
use widevine::device::{DeviceType, SecurityLevel};
use widevine::{Cdm, Device, LicenseType, Pssh};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub duration: f64,
}
impl Track {
    pub fn parse(v: &Value) -> Option<Self> {
        let a = &v["attributes"];
        let id = if v["type"] == "library-songs" {
            a["playParams"]["catalogId"].as_str()?
        } else {
            v["id"].as_str()?
        };
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(Self {
            id: id.into(),
            title: a["name"].as_str().unwrap_or("Без названия").into(),
            artist: a["artistName"].as_str().unwrap_or("").into(),
            duration: a["durationInMillis"].as_f64().unwrap_or(0.) / 1000.,
        })
    }
}
#[derive(Clone)]
pub struct Apple {
    pub http: Client,
    pub config: Arc<Config>,
    pub private: PathBuf,
    pub cache: PathBuf,
    pub storefront: String,
    gate: Arc<Mutex<tokio::time::Instant>>,
    prepare_gate: Arc<Mutex<()>>,
}
async fn response(r: Result<Response, reqwest::Error>) -> Result<Response> {
    let r = r.map_err(|e| anyhow!(e.without_url()))?;
    ensure!(
        r.status().is_success(),
        "Apple Music HTTP {}",
        r.status().as_u16()
    );
    Ok(r)
}
impl Apple {
    pub async fn new(config: Arc<Config>, private: PathBuf, cache: PathBuf) -> Result<Self> {
        tokio::fs::create_dir_all(&cache).await?;
        let http = Client::builder()
            .timeout(Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()?;
        let mut a = Self {
            http,
            config,
            private,
            cache,
            storefront: String::new(),
            gate: Arc::new(Mutex::new(
                tokio::time::Instant::now() - Duration::from_secs(1),
            )),
            prepare_gate: Arc::new(Mutex::new(())),
        };
        a.storefront = a.api("me/storefront", Method::GET, &[]).await?["data"][0]["id"]
            .as_str()
            .context("Missing Apple storefront")?
            .into();
        Ok(a)
    }
    pub async fn api(&self, path: &str, method: Method, params: &[(&str, &str)]) -> Result<Value> {
        ensure!(
            !path.starts_with('/') && !path.contains("://"),
            "Invalid Apple API path"
        );
        for attempt in 0..5 {
            let mut last = self.gate.lock().await;
            let wait = Duration::from_millis(300).saturating_sub(last.elapsed());
            tokio::time::sleep(wait).await;
            *last = tokio::time::Instant::now();
            let mut request = self
                .http
                .request(
                    method.clone(),
                    format!("https://api.music.apple.com/v1/{path}"),
                )
                .bearer_auth(&self.config.apple_app_token)
                .header("Music-User-Token", &self.config.apple_user_token)
                .query(params);
            if method == Method::POST {
                request = request.json(&json!({}));
            }
            let r = request.send().await.map_err(|e| anyhow!(e.without_url()))?;
            let code = r.status().as_u16();
            drop(last);
            if [429, 500, 502, 503, 504].contains(&code) {
                tokio::time::sleep(Duration::from_secs(1 << attempt.min(3))).await;
                continue;
            }
            if code == 401 || code == 403 {
                bail!("Нужно заново привязать Apple Music.");
            }
            return Ok(response(Ok(r)).await?.json().await?);
        }
        bail!("Apple Music временно ограничил запросы.")
    }
    pub async fn search(&self, query: &str, kind: &str) -> Result<Value> {
        self.api(
            &format!("catalog/{}/search", self.storefront),
            Method::GET,
            &[("term", query), ("types", kind), ("limit", "10")],
        )
        .await
    }
    pub async fn tracks(&self, path: String, params: Vec<(&str, &str)>) -> Result<Vec<Track>> {
        let mut path = path;
        let mut params = params;
        let mut result = vec![];
        let mut ids = HashSet::new();
        let mut pages = HashSet::new();
        while pages.insert(path.clone()) && result.len() < 500 {
            let data = self.api(&path, Method::GET, &params).await?;
            params.clear();
            for raw in data["data"].as_array().into_iter().flatten() {
                if let Some(t) = Track::parse(raw) {
                    if ids.insert(t.id.clone()) {
                        result.push(t);
                        if result.len() == 500 {
                            break;
                        }
                    }
                }
            }
            let Some(next) = data["next"].as_str() else {
                break;
            };
            ensure!(
                next.starts_with("/v1/") && !next.contains("://"),
                "Invalid Apple pagination URL"
            );
            path = next[4..].into();
        }
        Ok(result)
    }
    pub async fn select(&self, query: &str, mode: &str) -> Result<Vec<Track>> {
        if mode == "library" {
            return self
                .tracks("me/library/songs".into(), vec![("limit", "100")])
                .await;
        }
        let types = match mode {
            "play" => "songs",
            "album" => "albums",
            "playlist" => "playlists",
            "artist" => "artists",
            _ => "songs,artists",
        };
        let data = self.search(query, types).await?;
        let results = &data["results"];
        let artist = &results["artists"]["data"][0];
        let mut mode = mode;
        if mode == "auto"
            && normalize(query) == normalize(artist["attributes"]["name"].as_str().unwrap_or(""))
        {
            mode = "artist";
        }
        if mode == "play" || mode == "auto" {
            return Ok(Track::parse(&results["songs"]["data"][0])
                .into_iter()
                .collect());
        }
        let resource = match mode {
            "artist" => "artists",
            "album" => "albums",
            "playlist" => "playlists",
            _ => bail!("Unknown selection"),
        };
        let Some(id) = results[resource]["data"][0]["id"].as_str() else {
            return Ok(vec![]);
        };
        ensure!(
            id.bytes()
                .all(|x| x.is_ascii_alphanumeric() || b".-_".contains(&x)),
            "Invalid catalog ID"
        );
        let suffix = if mode == "artist" {
            "view/top-songs"
        } else {
            "tracks"
        };
        self.tracks(
            format!("catalog/{}/{resource}/{id}/{suffix}", self.storefront),
            vec![],
        )
        .await
    }
    pub async fn similar(&self, seed: &str) -> Result<Vec<Track>> {
        let data = self
            .api(
                &format!("me/stations/next-tracks/ra.{seed}"),
                Method::POST,
                &[("include", "artists")],
            )
            .await?;
        Ok(data["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Track::parse)
            .collect())
    }
    fn playback(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .post(url)
            .bearer_auth(&self.config.apple_app_token)
            .header("media-user-token", &self.config.apple_user_token)
            .header("origin", "https://music.apple.com")
            .header("referer", "https://music.apple.com/")
            .header("user-agent", "Mozilla/5.0")
    }
    async fn asset(&self, id: &str) -> Result<(String, Vec<u8>)> {
        let data: Value = response(
            self.playback("https://play.music.apple.com/WebObjects/MZPlay.woa/wa/webPlayback")
                .json(&json!({"salableAdamId":id}))
                .send()
                .await,
        )
        .await?
        .json()
        .await?;
        ensure!(
            data.get("failureType").is_none(),
            "Apple Music не разрешил воспроизведение."
        );
        let meta = &data["songList"][0];
        let asset = meta["assets"]
            .as_array()
            .and_then(|v| v.iter().find(|x| x["flavor"] == "28:ctrp256"))
            .context("No supported AAC asset")?;
        let mut url = reqwest::Url::parse(asset["URL"].as_str().context("Missing asset URL")?)?;
        let re = regex::Regex::new(r#"#EXT-X-KEY:.*?URI="([^"]+)""#)?;
        let mut uri = None;
        for _ in 0..4 {
            let text = response(self.http.get(url.clone()).send().await)
                .await?
                .text()
                .await?;
            if let Some(m) = re.captures(&text) {
                uri = Some(m[1].to_string());
            }
            let next = text
                .lines()
                .map(str::trim)
                .find(|s| !s.is_empty() && !s.starts_with('#'))
                .context("Empty playlist")?;
            url = url.join(next)?;
            if uri.is_some() {
                break;
            }
        }
        let uri = uri.context("No protection header")?;
        let key_id = B64.decode(uri.split_once(',').context("Invalid protection header")?.1)?;
        let key_pem = tokio::fs::read_to_string(self.private.join("private_key.pem")).await?;
        let key = RsaPrivateKey::from_pkcs1_pem(&key_pem)
            .or_else(|_| RsaPrivateKey::from_pkcs8_pem(&key_pem))
            .context("Invalid authorized client key")?;
        let device = Device::new(
            DeviceType::ANDROID,
            SecurityLevel::L3,
            key,
            &tokio::fs::read(self.private.join("client_id.bin")).await?,
        )
        .map_err(|_| anyhow!("Invalid authorized client profile"))?;
        let mut init = vec![8, 1, 18];
        put_len(&mut init, key_id.len());
        init.extend_from_slice(&key_id);
        let mut pssh_box = ((32 + init.len()) as u32).to_be_bytes().to_vec();
        pssh_box.extend(b"pssh");
        pssh_box.extend([0u8; 4]);
        pssh_box.extend([
            0xed, 0xef, 0x8b, 0xa9, 0x79, 0xd6, 0x4a, 0xce, 0xa3, 0xc8, 0x27, 0xdc, 0xd5, 0x1d,
            0x21, 0xed,
        ]);
        pssh_box.extend((init.len() as u32).to_be_bytes());
        pssh_box.extend(init);
        let pssh = Pssh::from_bytes(&pssh_box).map_err(|_| anyhow!("Invalid PSSH"))?;
        let request = Cdm::new(device)
            .open()
            .get_license_request(pssh, LicenseType::STREAMING)
            .map_err(|_| anyhow!("License request failed"))?;
        let challenge = B64.encode(
            request
                .challenge()
                .map_err(|_| anyhow!("License challenge failed"))?,
        );
        let license_url = meta["hls-key-server-url"]
            .as_str()
            .context("Missing license URL")?;
        ensure!(
            reqwest::Url::parse(license_url)?.scheme() == "https",
            "Invalid license URL"
        );
        let license:Value=response(self.playback(license_url).json(&json!({"challenge":challenge,"key-system":"com.widevine.alpha","uri":uri,"adamId":id,"isLibrary":false,"user-initiated":true})).send().await).await?.json().await?;
        let encoded = license["license"]
            .as_str()
            .context("Apple Music не выдал лицензию подписки.")?;
        let keys = request
            .get_keys(&B64.decode(encoded)?)
            .map_err(|_| anyhow!("License validation failed"))?;
        let key = keys
            .content_key(&key_id)
            .or_else(|_| keys.first_of_type(widevine::KeyType::CONTENT))
            .map_err(|_| anyhow!("No licensed content key"))?
            .key
            .clone();
        Ok((url.to_string(), key))
    }
    pub async fn prepare(&self, t: &Track) -> Result<PathBuf> {
        ensure!(
            !t.id.is_empty() && t.id.bytes().all(|x| x.is_ascii_digit()),
            "Invalid track ID"
        );
        let _lock = self.prepare_gate.lock().await;
        let destination = self.cache.join(format!("{}.m4a", t.id));
        if tokio::fs::metadata(&destination)
            .await
            .map(|x| x.len() > 1000)
            .unwrap_or(false)
        {
            return Ok(destination);
        }
        let (url, key) = self.asset(&t.id).await?;
        let input = self.cache.join(format!("{}.download", t.id));
        let output = self.cache.join(format!("{}.part.m4a", t.id));
        let result: Result<()> = async {
            let r = response(self.http.get(&url).send().await).await?;
            let mut stream = r.bytes_stream();
            let mut file = tokio::fs::File::create(&input).await?;
            let mut size = 0u64;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| anyhow!(e.without_url()))?;
                size += chunk.len() as u64;
                ensure!(
                    size <= self.config.max_download_bytes,
                    "Аудиофайл слишком большой."
                );
                file.write_all(&chunk).await?;
            }
            file.flush().await?;
            drop(file);
            let mut command = tokio::process::Command::new(&self.config.ffmpeg);
            command
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-decryption_key",
                    &hex::encode(key),
                    "-i",
                ])
                .arg(&input)
                .args(["-vn", "-c:a", "copy", "-movflags", "+faststart"])
                .arg(&output)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            let status = tokio::time::timeout(Duration::from_secs(60), command.status()).await??;
            ensure!(status.success(), "Не удалось подготовить AAC.");
            tokio::fs::rename(&output, &destination).await?;
            Ok(())
        }
        .await;
        let _ = tokio::fs::remove_file(input).await;
        let _ = tokio::fs::remove_file(output).await;
        result?;
        Ok(destination)
    }
    pub async fn trim(&self, keep: &[String]) -> Result<()> {
        let Ok(_lock) = self.prepare_gate.try_lock() else {
            return Ok(());
        };
        let mut entries = tokio::fs::read_dir(&self.cache).await?;
        while let Some(entry) = entries.next_entry().await? {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("m4a")
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()))
                && !keep
                    .iter()
                    .any(|id| p.file_stem().and_then(|s| s.to_str()) == Some(id))
            {
                let _ = tokio::fs::remove_file(p).await;
            }
        }
        Ok(())
    }
}
fn put_len(out: &mut Vec<u8>, mut n: usize) {
    while n >= 128 {
        out.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
pub fn normalize(s: &str) -> String {
    s.to_lowercase()
        .replace('ё', "е")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uploaded_file_is_not_a_catalog_track() {
        assert!(
            Track::parse(&json!({"type":"library-songs","id":"i.local","attributes":{}})).is_none()
        );
    }
    #[test]
    fn library_uses_catalog_id() {
        assert_eq!(
            Track::parse(
                &json!({"type":"library-songs","attributes":{"playParams":{"catalogId":"12"}}})
            )
            .unwrap()
            .id,
            "12"
        );
    }
    #[test]
    fn normalization_handles_cyrillic() {
        assert_eq!(normalize("Ёлка — LIVE"), "елка live");
    }
}
