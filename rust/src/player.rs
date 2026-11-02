use crate::{
    apple::{Apple, Track},
    audio::Audio,
    config::Config,
    volume::Volume,
    wire,
};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch, RwLock};
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
    pub airplay: bool,
    pub auto_advances: u64,
    pub error: Option<String>,
    pub last_started: Option<Instant>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            queue: Queue::default(),
            state: "paused".into(),
            airplay: false,
            auto_advances: 0,
            error: None,
            last_started: None,
        }
    }
}
#[derive(Clone)]
pub struct Shared {
    pub view: Arc<RwLock<View>>,
    pub volume: Volume,
    pub config: Arc<Config>,
    pub tx: mpsc::Sender<Job>,
    pub intent: watch::Sender<u64>,
    pub speech: watch::Sender<u64>,
    pub duck: watch::Sender<Option<Instant>>,
}
pub struct Job {
    pub action: String,
    pub query: String,
    pub voice: bool,
    pub reply: Option<oneshot::Sender<Result<String>>>,
    pub at: Instant,
    pub revision: Option<u64>,
}
fn music_intro(title: &str) -> String {
    let tags = regex::Regex::new(r"<[^>]{0,200}>").expect("static title pattern");
    let title = tags.replace_all(title, "");
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = title.chars().take(80).collect::<String>();
    let title = title.trim().trim_end_matches(['.', '!', '?']);
    format!(
        "Включаю {}.",
        if title.is_empty() {
            "песню"
        } else {
            title
        }
    )
}
async fn current_job<T>(
    mut intents: watch::Receiver<u64>,
    revision: u64,
    work: impl std::future::Future<Output = T>,
) -> Option<T> {
    if *intents.borrow_and_update() != revision {
        return None;
    }
    tokio::select! { biased; _=intents.changed()=>None, result=work=>Some(result) }
}
impl Shared {
    pub fn begin_intent(&self, action: &str) -> Option<u64> {
        if matches!(action, "announce_now" | "now" | "speak_text") {
            return None;
        }
        self.intent.send_modify(|r| *r = r.wrapping_add(1));
        Some(*self.intent.borrow())
    }
    pub async fn command(&self, action: &str, query: &str, voice: bool) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        let revision = self.begin_intent(action);
        self.tx
            .send(Job {
                action: action.into(),
                query: query.into(),
                voice,
                reply: Some(tx),
                at: Instant::now(),
                revision,
            })
            .await
            .map_err(|_| anyhow!("Player stopped"))?;
        tokio::time::timeout(Duration::from_secs(100), rx)
            .await?
            .map_err(|_| anyhow!("Player stopped"))?
    }
    pub async fn status(&self) -> Value {
        let v = self.view.read().await;
        let native = self.volume.snapshot();
        json!({"state":v.state,"current":v.queue.current,"next":v.queue.upcoming.iter().take(5).collect::<Vec<_>>(),"position":v.queue.position,"duration":v.queue.current.as_ref().map(|t|t.duration).unwrap_or(0.),"auto_advances":v.auto_advances,"airplay_active":v.airplay,"last_error":v.error,"volume":native["volume"],"playback_backend":"alsa","voice_connected":*self.speech.borrow()!=u64::MAX,"implementation":"rust","version":env!("CARGO_PKG_VERSION")})
    }
    pub fn speak(&self, text: String) {
        let this = self.clone();
        tokio::spawn(async move {
            let mark = Instant::now() + Duration::from_secs(65);
            this.duck.send_replace(Some(mark));
            let _ = wire::speak(this.config.recognition_port, &text).await;
            this.duck.send_if_modified(|v| {
                if *v == Some(mark) {
                    *v = None;
                    true
                } else {
                    false
                }
            });
        });
    }
}
type Recommendations = tokio::task::JoinHandle<Result<(String, Vec<Track>)>>;
pub struct Player {
    pub shared: Shared,
    apple: Apple,
    private: PathBuf,
    view: View,
    audio: Audio,
    prefetch: Option<tokio::task::JoinHandle<()>>,
    last_checkpoint: Instant,
    recommendations: Option<Recommendations>,
}
impl Player {
    pub fn new(apple: Apple, volume: Volume, private: PathBuf) -> (Self, mpsc::Receiver<Job>) {
        let queue = std::fs::read(private.join("queue.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let last_started = std::fs::metadata(private.join("queue.json"))
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|time| time.elapsed().ok())
            .filter(|age| *age < Duration::from_secs(3600))
            .map(|age| Instant::now() - age);
        let view = View {
            queue,
            last_started,
            ..View::default()
        };
        let (tx, rx) = mpsc::channel(8);
        let shared = Shared {
            view: Arc::new(RwLock::new(view.clone())),
            volume,
            config: apple.config.clone(),
            tx,
            intent: watch::channel(0).0,
            speech: watch::channel(u64::MAX).0,
            duck: watch::channel(None).0,
        };
        (
            Self {
                shared,
                apple,
                private,
                view,
                audio: Audio::default(),
                prefetch: None,
                last_checkpoint: Instant::now(),
                recommendations: None,
            },
            rx,
        )
    }
    async fn publish(&self) {
        let mut out = self.shared.view.write().await;
        let airplay = out.airplay;
        *out = self.view.clone();
        out.airplay = airplay;
    }
    async fn checkpoint(&mut self) -> Result<()> {
        let temp = self.private.join("queue.tmp");
        self.view.queue.history = self
            .view
            .queue
            .history
            .iter()
            .rev()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        tokio::fs::write(&temp, serde_json::to_vec(&self.view.queue)?).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600)).await?;
        }
        tokio::fs::rename(temp, self.private.join("queue.json")).await?;
        self.last_checkpoint = Instant::now();
        Ok(())
    }
    async fn handoff(&self) -> Result<()> {
        wire::pause_native().await
    }
    async fn play(&mut self, t: Track, position: f64, intro: Option<String>) -> Result<()> {
        let prepare = async {
            let at = Instant::now();
            let file = self.apple.prepare(&t).await?;
            println!("music_prepare elapsed_ms={}", at.elapsed().as_millis());
            Ok::<_, anyhow::Error>(file)
        };
        let handoff = async {
            let at = Instant::now();
            self.handoff().await?;
            if let Some(intro) = intro {
                self.shared.speak(intro);
            }
            println!("music_handoff elapsed_ms={}", at.elapsed().as_millis());
            Ok::<_, anyhow::Error>(())
        };
        let (file, ()) = tokio::try_join!(prepare, handoff)?;
        if self.shared.view.read().await.airplay {
            bail!("Сначала отключите AirPlay на телефоне.");
        }
        self.audio
            .start(
                &file,
                position,
                &self.shared.config,
                self.shared.volume.clone(),
                self.shared.duck.subscribe(),
            )
            .await?;
        self.view.queue.current = Some(t.clone());
        self.view.queue.position = position.max(0.);
        self.view.queue.seen.insert(t.id.clone());
        self.view.state = "playing".into();
        self.view.error = None;
        self.view.last_started = Some(Instant::now());
        self.checkpoint().await?;
        self.publish().await;
        println!("play song_id={} backend=alsa", t.id);
        let mut keep = vec![t.id];
        if let Some(t) = self.view.queue.upcoming.first() {
            keep.push(t.id.clone());
        }
        self.apple.trim(&keep).await?;
        Ok(())
    }
    async fn refill(&mut self) -> Result<()> {
        for _ in 0..3 {
            let seed = self
                .view
                .queue
                .seed
                .clone()
                .ok_or_else(|| anyhow!("Сначала назовите песню."))?;
            let mut ids: HashSet<_> = self
                .view
                .queue
                .upcoming
                .iter()
                .map(|t| t.id.clone())
                .collect();
            for t in self.apple.similar(&seed).await? {
                if !self.view.queue.seen.contains(&t.id) && ids.insert(t.id.clone()) {
                    self.view.queue.upcoming.push(t);
                }
            }
            if !self.view.queue.upcoming.is_empty() {
                return Ok(());
            }
        }
        bail!("Похожих песен пока нет.")
    }
    async fn prefetch(&mut self) {
        if self.prefetch.as_ref().is_some_and(|p| !p.is_finished()) {
            return;
        }
        if self.view.queue.upcoming.is_empty() {
            if self.recommendations.is_none() {
                if let Some(seed) = self.view.queue.seed.clone() {
                    let apple = self.apple.clone();
                    self.recommendations = Some(tokio::spawn(async move {
                        let tracks = apple.similar(&seed).await?;
                        Ok((seed, tracks))
                    }));
                }
            }
            return;
        }
        if let Some(t) = self.view.queue.upcoming.first().cloned() {
            let a = self.apple.clone();
            self.prefetch = Some(tokio::spawn(async move {
                let _ = a.prepare(&t).await;
            }));
        }
    }
    async fn next(&mut self) -> Result<String> {
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
        self.play(t, 0., None).await?;
        self.view.queue.upcoming.remove(0);
        if let Some(t) = old {
            self.view.queue.history.push(t);
        }
        self.checkpoint().await?;
        self.prefetch().await;
        Ok("Дальше.".into())
    }
    async fn execute(&mut self, j: &Job) -> Result<String> {
        if self.shared.view.read().await.airplay
            && !matches!(j.action.as_str(), "pause" | "stop" | "speak_text")
        {
            bail!("Сейчас играет AirPlay.");
        }
        match j.action.as_str() {
            "play" | "auto" | "artist" | "album" | "playlist" | "library" => {
                // A new selection supersedes speculative work for the old queue.
                if let Some(task) = self.prefetch.take() {
                    task.abort();
                    let _ = task.await;
                }
                if let Some(task) = self.recommendations.take() {
                    task.abort();
                    let _ = task.await;
                }
                let search_at = Instant::now();
                let mut tracks = self.apple.select(&j.query, &j.action).await?;
                println!(
                    "music_search elapsed_ms={}",
                    search_at.elapsed().as_millis()
                );
                if tracks.is_empty() {
                    bail!("В Apple Music ничего не найдено.");
                }
                let first = tracks.remove(0);
                let intro = j.voice.then(|| music_intro(&first.title));
                self.view.queue = Queue {
                    seed: Some(first.id.clone()),
                    upcoming: tracks,
                    ..Queue::default()
                };
                self.play(first, 0., intro).await?;
                self.prefetch().await;
                Ok("Включаю.".into())
            }
            "next" => self.next().await,
            "previous" => {
                let t = self
                    .view
                    .queue
                    .history
                    .last()
                    .cloned()
                    .ok_or_else(|| anyhow!("Предыдущей песни пока нет."))?;
                let old = self.view.queue.current.clone();
                self.play(t, 0., None).await?;
                self.view.queue.history.pop();
                if let Some(t) = old {
                    self.view.queue.upcoming.insert(0, t);
                }
                self.checkpoint().await?;
                self.prefetch().await;
                Ok("Предыдущая.".into())
            }
            "pause" | "stop" => {
                if self.audio.active() {
                    self.view.queue.position = self.audio.stop().await;
                }
                if j.voice {
                    self.handoff().await?;
                }
                self.view.state = "paused".into();
                self.checkpoint().await?;
                Ok("Пауза.".into())
            }
            "resume" => {
                if !self.audio.active() {
                    let t = self
                        .view
                        .queue
                        .current
                        .clone()
                        .ok_or_else(|| anyhow!("Сначала назовите песню."))?;
                    self.play(t, self.view.queue.position, None).await?;
                } else if j.voice {
                    self.handoff().await?;
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
                self.next().await
            }
            "speak_text" => {
                self.shared.speak(j.query.chars().take(700).collect());
                Ok("Готово.".into())
            }
            "now" | "announce_now" => {
                let t = self
                    .view
                    .queue
                    .current
                    .as_ref()
                    .ok_or_else(|| anyhow!("Пока ничего не играет."))?;
                let text = format!("{} — {}", t.title, t.artist);
                if j.action == "announce_now" {
                    self.shared.speak(text.clone());
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
                let position = j.query.parse::<f64>()?.clamp(0., duration);
                if self.audio.active() {
                    let t = self
                        .view
                        .queue
                        .current
                        .clone()
                        .ok_or_else(|| anyhow!("Сначала назовите песню."))?;
                    self.play(t, position, None).await?;
                } else {
                    self.view.queue.position = position;
                }
                self.checkpoint().await?;
                Ok("Готово.".into())
            }
            _ => bail!("Неизвестная команда."),
        }
    }
    async fn handle_job(&mut self, job: Job) {
        if job.reply.as_ref().is_some_and(|r| r.is_closed())
            || job.at.elapsed() > Duration::from_secs(30)
        {
            return;
        }
        let before = self.view.clone();
        let result = if let Some(revision) = job.revision {
            let intents = self.shared.intent.subscribe();
            match current_job(intents, revision, self.execute(&job)).await {
                Some(result) => result,
                None => {
                    self.audio.stop().await;
                    self.view = before;
                    self.view.state = "paused".into();
                    self.publish().await;
                    if let Some(reply) = job.reply {
                        let _ = reply.send(Err(anyhow!("Команда заменена новой.")));
                    }
                    return;
                }
            }
        } else {
            self.execute(&job).await
        };
        if result.is_ok()
            && job.voice
            && !matches!(job.action.as_str(), "speak_text" | "announce_now" | "now")
        {
            println!(
                "music_command action={} elapsed_ms={}",
                job.action,
                job.at.elapsed().as_millis()
            );
        }
        if result.is_err() {
            self.view = before;
            self.view.state = if self.audio.active() {
                "playing".into()
            } else {
                "paused".into()
            };
            self.view.error = Some("Не удалось выполнить музыкальную команду.".into());
            eprintln!("command_failed action={}", job.action);
        }
        self.publish().await;
        if let Some(reply) = job.reply {
            let _ = reply.send(result);
        }
        // Like the recipe, playback failures are logged, not a second spoken reply.
    }
    async fn observe(&mut self) -> Result<()> {
        if self
            .recommendations
            .as_ref()
            .is_some_and(|task| task.is_finished())
        {
            if let Some(task) = self.recommendations.take() {
                if let Ok(Ok((seed, tracks))) = task.await {
                    if self.view.queue.seed.as_deref() == Some(seed.as_str()) {
                        let mut ids: HashSet<_> = self
                            .view
                            .queue
                            .upcoming
                            .iter()
                            .map(|t| t.id.clone())
                            .collect();
                        for track in tracks {
                            if !self.view.queue.seen.contains(&track.id)
                                && ids.insert(track.id.clone())
                            {
                                self.view.queue.upcoming.push(track);
                            }
                        }
                        if !self.view.queue.upcoming.is_empty() {
                            self.prefetch().await;
                        }
                    }
                }
            }
        }

        if self.shared.view.read().await.airplay && self.audio.active() {
            self.view.queue.position = self.audio.stop().await;
            self.view.state = "paused".into();
            self.checkpoint().await?;
        }
        if self.audio.active() {
            self.view.queue.position = self.audio.position();
        }
        if self.audio.finished().await? && self.view.state == "playing" {
            self.view.state = "paused".into();
            if self.next().await.is_ok() {
                self.view.auto_advances += 1;
            }
        }
        if self.view.state == "playing" && self.last_checkpoint.elapsed() > Duration::from_secs(60)
        {
            self.checkpoint().await?;
        }
        Ok(())
    }
    pub async fn run(mut self, mut rx: mpsc::Receiver<Job>) {
        self.publish().await;
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                job=rx.recv()=>match job{Some(job)=>self.handle_job(job).await,None=>break},
                _=tick.tick()=>{if self.observe().await.is_err(){self.audio.stop().await;self.view.state="paused".into();self.view.error=Some("Ошибка локального аудиовыхода.".into());}self.publish().await;}
            }
        }
        self.audio.stop().await;
    }
}

#[cfg(test)]
mod intro_tests {
    use super::*;
    #[test]
    fn short_intro_contains_only_the_song_title() {
        assert_eq!(music_intro("Babydoll"), "Включаю Babydoll.");
        assert_eq!(
            music_intro("<audio src='x'/> \n Танцуй сама!"),
            "Включаю Танцуй сама."
        );
        assert_eq!(music_intro(""), "Включаю песню.");
        assert!(music_intro(&"я".repeat(500)).chars().count() <= 89);
    }
}
