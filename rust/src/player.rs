use crate::{
    apple::{Apple, Track},
    config::Config,
    station::Station,
    wire,
};
use anyhow::{anyhow, bail, Result};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, RwLock};
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Queue {
    #[serde(default)]
    pub current: Option<Track>,
    #[serde(default, rename = "next")]
    pub upcoming: Vec<Track>,
    #[serde(default)]
    pub history: Vec<Track>,
    #[serde(default)]
    pub seen: HashSet<String>,
    #[serde(default)]
    pub seed: Option<String>,
    #[serde(default)]
    pub position: f64,
}
#[derive(Clone)]
pub struct View {
    pub queue: Queue,
    pub state: String,
    pub stream_id: String,
    pub airplay: bool,
    pub auto_advances: u64,
    pub fetch_count: u64,
    pub error: Option<String>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            queue: Queue::default(),
            state: "paused".into(),
            stream_id: String::new(),
            airplay: false,
            auto_advances: 0,
            fetch_count: 0,
            error: None,
        }
    }
}
#[derive(Clone)]
pub struct Shared {
    pub view: Arc<RwLock<View>>,
    pub station: Station,
    pub config: Arc<Config>,
    pub secret: [u8; 32],
    pub cache: PathBuf,
    pub tx: mpsc::Sender<Job>,
}
pub struct Job {
    pub action: String,
    pub query: String,
    pub voice: bool,
    pub reply: Option<oneshot::Sender<Result<String>>>,
    pub at: Instant,
}
impl Shared {
    pub fn token(&self, id: &str) -> String {
        let mut m = Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC");
        m.update(id.as_bytes());
        hex::encode(m.finalize().into_bytes())
    }
    pub fn verify(&self, id: &str, token: &str) -> bool {
        let Ok(t) = hex::decode(token) else {
            return false;
        };
        let mut m = Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC");
        m.update(id.as_bytes());
        m.verify_slice(&t).is_ok()
    }
    pub async fn command(&self, action: &str, query: &str, voice: bool) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Job {
                action: action.into(),
                query: query.into(),
                voice,
                reply: Some(tx),
                at: Instant::now(),
            })
            .await
            .map_err(|_| anyhow!("Player stopped"))?;
        tokio::time::timeout(Duration::from_secs(100), rx)
            .await?
            .map_err(|_| anyhow!("Player stopped"))?
    }
    pub async fn status(&self) -> Value {
        let v = self.view.read().await;
        let s = self.station.snapshot();
        json!({"state":v.state,"current":v.queue.current,"next":v.queue.upcoming.iter().take(5).collect::<Vec<_>>(),"position":v.queue.position,"duration":v.queue.current.as_ref().map(|t|t.duration).unwrap_or(0.),"station_connected":s.get("aliceState").is_some(),"station_alice":s["aliceState"],"station_playing":s["playing"],"station_stream_id":s["playerState"]["id"],"auto_advances":v.auto_advances,"fetch_count":v.fetch_count,"airplay_active":v.airplay,"last_error":v.error,"implementation":"rust","version":env!("CARGO_PKG_VERSION")})
    }
}
pub struct Player {
    pub shared: Shared,
    apple: Apple,
    private: PathBuf,
    view: View,
    generation: u64,
    prefetch: Option<tokio::task::JoinHandle<()>>,
    grace: Instant,
    last_playing: bool,
    last_alice: String,
    last_progress: f64,
    last_checkpoint: Instant,
    volume_history: std::collections::VecDeque<(Instant, f64)>,
    volume_recovery: Option<(Instant, f64)>,
    title_recover_until: Option<Instant>,
}
impl Player {
    pub fn new(apple: Apple, station: Station, private: PathBuf) -> (Self, mpsc::Receiver<Job>) {
        let queue = std::fs::read(private.join("queue.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let view = View {
            queue,
            ..View::default()
        };
        let (tx, rx) = mpsc::channel(8);
        let shared = Shared {
            view: Arc::new(RwLock::new(view.clone())),
            station,
            config: apple.config.clone(),
            secret: rand::random(),
            cache: apple.cache.clone(),
            tx,
        };
        (
            Self {
                shared,
                apple,
                private,
                view,
                generation: 0,
                prefetch: None,
                grace: Instant::now() + Duration::from_secs(4),
                last_playing: false,
                last_alice: "IDLE".into(),
                last_progress: 0.,
                last_checkpoint: Instant::now(),
                volume_history: std::collections::VecDeque::new(),
                volume_recovery: None,
                title_recover_until: None,
            },
            rx,
        )
    }
    async fn publish(&self) {
        let mut output = self.shared.view.write().await;
        let fetch = output.fetch_count;
        let airplay = output.airplay;
        *output = self.view.clone();
        output.fetch_count = fetch;
        output.airplay = airplay;
    }
    async fn checkpoint(&mut self) -> Result<()> {
        let temp = self.private.join("queue.tmp");
        let mut queue = self.view.queue.clone();
        queue.history = queue
            .history
            .into_iter()
            .rev()
            .take(30)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        tokio::fs::write(&temp, serde_json::to_vec(&queue)?).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600)).await?;
        }
        tokio::fs::rename(temp, self.private.join("queue.json")).await?;
        self.last_checkpoint = Instant::now();
        Ok(())
    }
    async fn wait_idle(&self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut idle = None;
        while Instant::now() < deadline {
            let s = self.shared.station.snapshot();
            if s["aliceState"] == "IDLE" {
                if idle.is_none() {
                    idle = Some(Instant::now());
                }
                if idle.is_some_and(|t| t.elapsed() > Duration::from_millis(800)) {
                    return Ok(());
                }
            } else {
                idle = None;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        bail!("Колонка ещё слушает. Повторите команду.")
    }
    async fn play(&mut self, track: Track, voice: bool, position: f64) -> Result<()> {
        self.apple.prepare(&track).await?;
        if voice {
            self.wait_idle().await?;
        }
        if self.shared.view.read().await.airplay {
            bail!("Сначала отключите AirPlay на телефоне.");
        }
        self.generation += 1;
        let id = format!("alice-music-rust:{}:{}", track.id, self.generation);
        let url = format!(
            "http://127.0.0.1:{}/audio/{}/{}.m4a",
            self.shared.config.port,
            self.shared.token(&track.id),
            track.id
        );
        for attempt in 0..3 {
            self.shared.station.play(&url, &track, &id).await?;
            tokio::time::sleep(Duration::from_millis(700)).await;
            let _ = self
                .shared
                .station
                .send(json!({"command":"softwareVersion"}))
                .await;
            let status = self.shared.station.snapshot();
            if status["playing"] == true && status["playerState"]["id"] == id {
                break;
            }
            if attempt == 2 {
                bail!("Станция не начала воспроизведение.");
            }
            if voice {
                self.wait_idle().await?;
            }
        }
        self.view.stream_id = id;
        self.view.queue.current = Some(track.clone());
        self.view.queue.position = position.max(0.);
        self.last_progress = position.max(0.);
        self.view.queue.seen.insert(track.id.clone());
        self.view.state = "playing".into();
        self.view.error = None;
        self.grace = Instant::now() + Duration::from_secs(5);
        if position > 1. {
            tokio::time::sleep(Duration::from_millis(500)).await;
            self.shared
                .station
                .send(json!({"command":"rewind","position":position}))
                .await?;
        }
        self.checkpoint().await?;
        self.publish().await;
        println!("play song_id={}", track.id);
        let mut keep = vec![track.id];
        if let Some(t) = self.view.queue.upcoming.first() {
            keep.push(t.id.clone());
        }
        self.apple.trim(&keep).await?;
        Ok(())
    }
    async fn refill(&mut self) -> Result<()> {
        for _ in 0..3 {
            let Some(seed) = self.view.queue.seed.clone() else {
                bail!("Сначала назовите песню.")
            };
            let tracks = self.apple.similar(&seed).await?;
            let mut ids: HashSet<_> = self
                .view
                .queue
                .upcoming
                .iter()
                .map(|t| t.id.clone())
                .collect();
            for t in tracks {
                if !self.view.queue.seen.contains(&t.id) && ids.insert(t.id.clone()) {
                    self.view.queue.upcoming.push(t);
                }
            }
            if !self.view.queue.upcoming.is_empty() {
                return Ok(());
            }
            self.view.queue.seed = self.view.queue.current.as_ref().map(|t| t.id.clone());
        }
        bail!("Apple Music не подобрал новые песни.")
    }
    async fn prefetch(&mut self) {
        if self.prefetch.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        if self.view.queue.upcoming.is_empty() && self.refill().await.is_err() {
            return;
        }
        if let Some(t) = self.view.queue.upcoming.first().cloned() {
            let a = self.apple.clone();
            self.prefetch = Some(tokio::spawn(async move {
                let _ = a.prepare(&t).await;
            }));
        }
    }
    async fn next(&mut self, voice: bool) -> Result<String> {
        if self.view.queue.upcoming.is_empty() {
            self.refill().await?;
        }
        let t = self
            .view
            .queue
            .upcoming
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("Нет следующей песни."))?;
        let old = self.view.queue.current.clone();
        self.play(t, voice, 0.).await?;
        self.view.queue.upcoming.remove(0);
        if let Some(old) = old {
            self.view.queue.history.push(old);
        }
        self.checkpoint().await?;
        self.prefetch().await;
        Ok("Дальше.".into())
    }
    async fn execute(&mut self, j: &Job) -> Result<String> {
        if self.shared.view.read().await.airplay && !matches!(j.action.as_str(), "pause" | "stop") {
            bail!("Сейчас играет AirPlay.");
        }
        match j.action.as_str() {
            "play" | "auto" | "artist" | "album" | "playlist" | "library" => {
                let mut tracks = self.apple.select(&j.query, &j.action).await?;
                if tracks.is_empty() {
                    bail!("В Apple Music ничего не найдено.");
                }
                let first = tracks.remove(0);
                self.view.queue = Queue {
                    seed: Some(first.id.clone()),
                    upcoming: tracks,
                    ..Queue::default()
                };
                self.play(first, j.voice, 0.).await?;
                self.prefetch().await;
                Ok("Включаю.".into())
            }
            "next" => self.next(j.voice).await,
            "previous" => {
                let t = self
                    .view
                    .queue
                    .history
                    .last()
                    .cloned()
                    .ok_or_else(|| anyhow!("Предыдущей песни пока нет."))?;
                let old = self.view.queue.current.clone();
                self.play(t, j.voice, 0.).await?;
                self.view.queue.history.pop();
                if let Some(t) = old {
                    self.view.queue.upcoming.insert(0, t);
                }
                self.checkpoint().await?;
                self.prefetch().await;
                Ok("Предыдущая.".into())
            }
            "pause" | "stop" => {
                self.view.state = "paused".into();
                self.title_recover_until = None;
                self.volume_recovery = None;
                self.generation += 1;
                self.shared.station.send(json!({"command":"stop"})).await?;
                self.checkpoint().await?;
                Ok("Пауза.".into())
            }
            "resume" => {
                let t = self
                    .view
                    .queue
                    .current
                    .clone()
                    .ok_or_else(|| anyhow!("Сначала назовите песню."))?;
                let s = self.shared.station.snapshot();
                if s["playing"] == true && s["playerState"]["id"] == self.view.stream_id {
                    self.view.state = "playing".into();
                } else {
                    self.play(t, j.voice, self.view.queue.position).await?;
                }
                self.prefetch().await;
                Ok("Продолжаю.".into())
            }
            "similar" => {
                let t = self
                    .view
                    .queue
                    .current
                    .as_ref()
                    .ok_or_else(|| anyhow!("Сначала назовите песню."))?;
                self.view.queue.seed = Some(t.id.clone());
                self.view.queue.upcoming.clear();
                self.view.queue.seen = HashSet::from([t.id.clone()]);
                self.next(j.voice).await
            }
            "now" | "announce_now" => {
                let t = self
                    .view
                    .queue
                    .current
                    .as_ref()
                    .ok_or_else(|| anyhow!("Музыка ещё не включена."))?;
                let text = format!("{}, {}", t.title, t.artist);
                let port = self.shared.config.recognition_port;
                let spoken = text.clone();
                tokio::spawn(async move {
                    let _ = wire::speak(port, &spoken).await;
                });
                if self.view.state == "playing" {
                    self.grace = Instant::now() + Duration::from_secs(3);
                    self.title_recover_until = Some(Instant::now() + Duration::from_secs(30));
                }
                Ok(text)
            }
            "seek" => {
                let duration = self
                    .view
                    .queue
                    .current
                    .as_ref()
                    .map(|t| t.duration)
                    .unwrap_or(0.);
                let pos = j.query.parse::<f64>()?.clamp(0., duration);
                self.shared
                    .station
                    .send(json!({"command":"rewind","position":pos}))
                    .await?;
                self.view.queue.position = pos;
                self.last_progress = pos;
                self.grace = Instant::now() + Duration::from_secs(4);
                Ok("Готово.".into())
            }
            _ => bail!("Неизвестная команда."),
        }
    }
    async fn handle_job(&mut self, job: Job) {
        if job.at.elapsed() > Duration::from_secs(30) {
            if let Some(reply) = job.reply {
                let _ = reply.send(Err(anyhow!("Команда устарела.")));
            }
            return;
        }
        let before = self.view.clone();
        let volume = if job.voice {
            let cutoff = Instant::now() - Duration::from_secs(3);
            self.volume_history
                .iter()
                .rev()
                .find(|(at, _)| *at <= cutoff)
                .map(|(_, volume)| *volume)
                .or_else(|| self.shared.station.snapshot()["volume"].as_f64())
        } else {
            None
        };
        let result = self.execute(&job).await;
        if result.is_ok() && job.voice && self.view.state == "playing" {
            if let Some(volume) = volume {
                self.volume_recovery = Some((Instant::now() + Duration::from_secs(20), volume));
            }
        }
        if result.is_err() {
            self.view = before;
            self.view.error = Some("Не удалось выполнить музыкальную команду.".into());
            eprintln!("command_failed action={}", job.action);
        }
        self.publish().await;
        if let Some(reply) = job.reply {
            let _ = reply.send(result);
        } else if result.is_err() {
            let port = self.shared.config.recognition_port;
            tokio::spawn(async move {
                let _ = wire::speak(port, "Не получилось. Повторите запрос.").await;
            });
        }
    }

    pub async fn run(mut self, mut rx: mpsc::Receiver<Job>) {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        self.publish().await;
        loop {
            tokio::select! {
                job = rx.recv() => match job {
                    Some(job) => self.handle_job(job).await,
                    None => break,
                },
                _ = tick.tick() => {
                    if self.observe().await.is_err() {
                        eprintln!("playback_observer_error");
                    }
                    self.publish().await;
                }
            }
        }
    }
    async fn observe(&mut self) -> Result<()> {
        let s = self.shared.station.snapshot();
        let playing = s["playing"] == true;
        let alice = s["aliceState"].as_str().unwrap_or("");
        let own = s["playerState"]["id"].as_str() == Some(self.view.stream_id.as_str())
            && !self.view.stream_id.is_empty();
        let progress = s["playerState"]["progress"].as_f64().unwrap_or(0.);
        if alice == "IDLE" {
            if let Some(volume) = s["volume"].as_f64() {
                self.volume_history.push_back((Instant::now(), volume));
                while self.volume_history.len() > 120 {
                    self.volume_history.pop_front();
                }
            }
        }
        if let Some((until, volume)) = self.volume_recovery {
            if Instant::now() > until {
                self.volume_recovery = None;
            } else if own && alice == "IDLE" && Instant::now() > self.grace {
                self.volume_recovery = None;
                if let Some(current) = s["volume"].as_f64() {
                    if volume > 0. && current < volume && (current - volume / 2.).abs() <= 0.015 {
                        self.shared
                            .station
                            .send(json!({"command":"setVolume","volume":volume}))
                            .await?;
                    }
                }
            }
        }
        if own && playing && progress >= 0. {
            self.view.queue.position = progress;
            self.last_progress = progress;
        }
        if self.shared.view.read().await.airplay && self.view.state == "playing" {
            self.view.state = "paused".into();
            self.title_recover_until = None;
            self.shared.station.send(json!({"command":"stop"})).await?;
            self.checkpoint().await?;
        }
        if let Some(until) = self.title_recover_until {
            if Instant::now() > until {
                self.title_recover_until = None;
            } else if own && !playing && alice == "IDLE" && Instant::now() > self.grace {
                self.title_recover_until = None;
                if let Some(t) = self.view.queue.current.clone() {
                    self.play(t, false, self.view.queue.position).await?;
                }
            }
        }
        if own
            && self.view.state == "playing"
            && self.last_playing
            && !playing
            && alice == "IDLE"
            && self.last_alice == "IDLE"
            && Instant::now() > self.grace
        {
            let duration = self
                .view
                .queue
                .current
                .as_ref()
                .map(|t| t.duration)
                .unwrap_or(f64::MAX);
            if self.last_progress >= duration - 3. {
                if self.next(false).await.is_ok() {
                    self.view.auto_advances += 1;
                }
            } else if self.title_recover_until.is_none() {
                self.view.state = "paused".into();
            }
        }
        self.last_playing = playing && own;
        self.last_alice = alice.into();
        if self.view.state == "playing" && self.last_checkpoint.elapsed() > Duration::from_secs(60)
        {
            self.checkpoint().await?;
        }
        Ok(())
    }
}
