//! Local PCM playback: FFmpeg decodes, the firmware's aplay owns the stream.
//! No Glagol transport commands or playback-status retries.
use crate::{config::Config, volume::Volume};
use anyhow::{anyhow, bail, ensure, Result};
use std::{
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::watch,
};

pub struct Audio {
    decoder: Option<Child>,
    sink: Option<Child>,
    pump: Option<tokio::task::JoinHandle<Result<()>>>,
    frames: Arc<AtomicU64>,
    base: f64,
}
impl Default for Audio {
    fn default() -> Self {
        Self {
            decoder: None,
            sink: None,
            pump: None,
            frames: Arc::new(AtomicU64::new(0)),
            base: 0.,
        }
    }
}
pub fn volume_gain(volume: Option<f64>, limit: f64) -> f64 {
    let Some(v) = volume.filter(|v| v.is_finite()) else {
        return 0.;
    };
    v.clamp(0., 1.).min(limit.clamp(0., 1.))
}
fn attenuate(pcm: &mut [u8], gain: f64) {
    for sample in pcm.as_chunks_mut::<2>().0 {
        let x = i16::from_le_bytes([sample[0], sample[1]]);
        sample.copy_from_slice(&((x as f64 * gain).round() as i16).to_le_bytes());
    }
}
impl Audio {
    pub fn position(&self) -> f64 {
        self.base + self.frames.load(Ordering::Relaxed) as f64 / 48000.
    }
    pub fn active(&self) -> bool {
        self.sink.is_some()
    }
    pub async fn stop(&mut self) -> f64 {
        let position = self.position();
        if let Some(pump) = self.pump.take() {
            pump.abort();
            let _ = pump.await;
        }
        for child in [&mut self.decoder, &mut self.sink] {
            if let Some(mut process) = child.take() {
                let _ = process.kill().await;
            }
        }
        self.base = position;
        self.frames.store(0, Ordering::Relaxed);
        position
    }
    pub async fn start(
        &mut self,
        file: &Path,
        position: f64,
        c: &Config,
        volume: Volume,
        duck: watch::Receiver<Option<Instant>>,
    ) -> Result<()> {
        self.stop().await;
        self.base = position.max(0.);
        self.frames.store(0, Ordering::Relaxed);
        let mut decoder = Command::new(&c.ffmpeg);
        decoder
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                &self.base.to_string(),
                "-i",
            ])
            .arg(file)
            .args(["-vn", "-f", "s16le", "-ac", "2", "-ar", "48000", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut decoder = decoder.spawn()?;
        let mut input = decoder
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Missing PCM input"))?;
        let mut sink = Command::new(&c.aplay);
        sink.args([
            "-q",
            "-D",
            &c.pcm,
            "-f",
            "S16_LE",
            "-c",
            "2",
            "-r",
            "48000",
            "-t",
            "raw",
            "--buffer-time=100000",
        ])
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("LD_PRELOAD")
        .env_remove("ALSA_CONFIG_PATH")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
        let mut sink = sink.spawn()?;
        let mut output = sink
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Missing PCM output"))?;
        let frames = self.frames.clone();
        let limit = c.audio_volume_limit;
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        self.decoder = Some(decoder);
        self.sink = Some(sink);
        self.pump = Some(tokio::spawn(async move {
            let mut ready = Some(ready_tx);
            let mut pcm = [0u8; 8192];
            let mut retained = 0;
            loop {
                let n = input.read(&mut pcm[retained..]).await?;
                if n == 0 {
                    ensure!(retained == 0, "Incomplete PCM frame");
                    break;
                }
                let length = retained + n;
                let complete = length / 4 * 4;
                let tail = pcm[complete..length].to_vec();
                let mut gain = volume_gain(volume.snapshot()["volume"].as_f64(), limit);
                if duck.borrow().is_some_and(|until| until > Instant::now()) {
                    gain *= 0.25;
                }
                attenuate(&mut pcm[..complete], gain);
                output.write_all(&pcm[..complete]).await?;
                frames.fetch_add((complete / 4) as u64, Ordering::Relaxed);
                if complete != 0 {
                    if let Some(tx) = ready.take() {
                        let _ = tx.send(());
                    }
                }
                retained = tail.len();
                pcm[..retained].copy_from_slice(&tail);
            }
            output.shutdown().await?;
            Ok(())
        }));
        match tokio::time::timeout(Duration::from_secs(4), ready_rx).await {
            Ok(Ok(())) => Ok(()),
            _ => {
                self.stop().await;
                bail!("Не удалось открыть локальный аудиовыход.")
            }
        }
    }
    /// True once the sink has drained; this is the queue's only end-of-track signal.
    pub async fn finished(&mut self) -> Result<bool> {
        if let Some(sink) = &mut self.sink {
            if let Some(status) = sink.try_wait()? {
                let pumped = if let Some(pump) = self.pump.take() {
                    pump.await?
                } else {
                    Ok(())
                };
                let decoded = if let Some(mut decoder) = self.decoder.take() {
                    decoder.wait().await?.success()
                } else {
                    false
                };
                self.sink = None;
                pumped?;
                ensure!(
                    status.success() && decoded,
                    "Локальный аудиопоток завершился с ошибкой."
                );
                return Ok(true);
            }
        }
        Ok(false)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_volume_is_silent() {
        assert_eq!(volume_gain(None, 1.), 0.);
        assert_eq!(volume_gain(Some(f64::NAN), 1.), 0.);
    }
    #[test]
    fn volume_one_is_attenuated_and_respects_test_limit() {
        assert_eq!(volume_gain(Some(0.1), 1.), 0.1);
        assert_eq!(volume_gain(Some(1.), 0.1), volume_gain(Some(0.1), 1.));
        assert_eq!(volume_gain(Some(0.), 1.), 0.);
    }
    #[test]
    fn pcm_gain_preserves_sign_and_silence() {
        let mut pcm = [
            i16::MAX.to_le_bytes(),
            i16::MIN.to_le_bytes(),
            0i16.to_le_bytes(),
        ]
        .concat();
        attenuate(&mut pcm, 0.25);
        assert_eq!(i16::from_le_bytes(pcm[..2].try_into().unwrap()), 8192);
        assert_eq!(i16::from_le_bytes(pcm[2..4].try_into().unwrap()), -8192);
        assert_eq!(&pcm[4..], &[0, 0]);
    }
}

#[cfg(all(test, unix))]
mod process_tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    async fn script(path: &Path, body: &str) {
        tokio::fs::write(path, format!("#!/bin/sh\n{body}\n"))
            .await
            .unwrap();
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn decoded_pcm_reaches_sink_and_eof_advances_once() {
        let dir = std::env::temp_dir().join(format!("apple-pcm-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&dir).await.unwrap();
        script(&dir.join("decoder"), "printf '\\377\\177\\000\\200'").await;
        script(&dir.join("sink"), "exec cat > \"$0.out\"").await;
        let c:Config=serde_json::from_value(json!({"apple_app_token":"fixture","apple_user_token":"fixture","yandex_music_token":"fixture","yandex_x_token":"fixture","station_device_id":"fixture","station_platform":"fixture","agent_token":"fixture","ffmpeg":dir.join("decoder"),"aplay":dir.join("sink")})).unwrap();
        let mut audio = Audio::default();
        let (_tx, rx) = watch::channel(None);
        audio
            .start(&dir.join("unused.m4a"), 0., &c, Volume::fixture(), rx)
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            if audio.finished().await.unwrap() {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!audio.finished().await.unwrap());
        assert!(!audio.active());
        let pcm = tokio::fs::read(dir.join("sink.out")).await.unwrap();
        assert_eq!(pcm.len(), 4);
        assert!((i16::from_le_bytes(pcm[..2].try_into().unwrap()) - 3277).abs() <= 1);
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }
    #[tokio::test]
    async fn pause_reaps_both_processes_and_preserves_position() {
        let dir = std::env::temp_dir().join(format!("apple-pcm-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&dir).await.unwrap();
        script(
            &dir.join("decoder"),
            "exec awk 'BEGIN {while (1) {printf \"abcdabcd\";fflush();}}'",
        )
        .await;
        script(&dir.join("sink"), "exec cat > /dev/null").await;
        let c:Config=serde_json::from_value(json!({"apple_app_token":"fixture","apple_user_token":"fixture","yandex_music_token":"fixture","yandex_x_token":"fixture","station_device_id":"fixture","station_platform":"fixture","agent_token":"fixture","ffmpeg":dir.join("decoder"),"aplay":dir.join("sink")})).unwrap();
        let mut audio = Audio::default();
        let (_tx, rx) = watch::channel(None);
        audio
            .start(&dir.join("unused.m4a"), 12., &c, Volume::fixture(), rx)
            .await
            .unwrap();
        let position = audio.stop().await;
        assert!(position >= 12.);
        assert!(!audio.active());
        assert!(audio.decoder.is_none() && audio.pump.is_none());
        assert_eq!(audio.position(), position);
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }
}
