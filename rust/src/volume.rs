//! Read-only native volume. Missing, corrupt or out-of-range state means mute.
//! Midi's platform scale is 0..20 (voice levels 0..10, button steps 5%).
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tokio::{io::AsyncReadExt, sync::watch};
#[derive(Clone)]
pub struct Volume {
    state: watch::Receiver<Value>,
}
fn parse(bytes: &[u8], max: f64) -> Option<f64> {
    if bytes.len() > 64 || !max.is_finite() || max <= 0. {
        return None;
    }
    let level = std::str::from_utf8(bytes)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()?;
    (level.is_finite() && (0.0..=max).contains(&level)).then_some(level / max)
}
impl Volume {
    pub fn start(path: PathBuf, max: f64) -> Self {
        let (tx, state) = watch::channel(json!({}));
        tokio::spawn(async move {
            loop {
                let read: std::io::Result<Vec<u8>> = async {
                    let file = tokio::fs::File::open(&path).await?;
                    let mut bytes = vec![];
                    file.take(65).read_to_end(&mut bytes).await?;
                    Ok(bytes)
                }
                .await;
                let volume = read.ok().and_then(|b| parse(&b, max));
                tx.send_replace(json!({"volume":volume}));
                tokio::select! {_=tx.closed()=>break,_=tokio::time::sleep(Duration::from_millis(100))=>{}}
            }
        });
        Self { state }
    }
    pub fn snapshot(&self) -> Value {
        self.state.borrow().clone()
    }
    #[cfg(test)]
    pub fn fixture() -> Self {
        let (_tx, state) = watch::channel(json!({"volume":0.1}));
        Self { state }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_zero_is_mute() {
        assert_eq!(parse(b"0", 20.), Some(0.));
    }
    #[test]
    fn native_scale_and_invalid_states() {
        assert_eq!(parse(b"2\n", 20.), Some(0.1));
        assert_eq!(parse(b"20", 20.), Some(1.));
        for data in [b"".as_slice(), b"NaN", b"21", b"-1", b"junk"] {
            assert_eq!(parse(data, 20.), None);
        }
    }
    #[tokio::test]
    async fn watcher_follows_changes_and_mutes_after_removal() {
        let path = std::env::temp_dir().join(format!("apple-volume-{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&path, b"2").await.unwrap();
        let volume = Volume::start(path.clone(), 20.);
        for _ in 0..20 {
            if volume.snapshot()["volume"] == 0.1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(volume.snapshot()["volume"], 0.1);
        tokio::fs::write(&path, b"0").await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(volume.snapshot()["volume"], 0.);
        tokio::fs::remove_file(&path).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(volume.snapshot()["volume"].is_null());
    }
}
