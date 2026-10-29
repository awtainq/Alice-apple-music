use crate::{
    player::{Job, Shared},
    wire,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncSeekExt},
    sync::Mutex,
};

pub fn parse(text: &str, recent: bool) -> Option<(String, String)> {
    let text = text.to_lowercase().replace('ё', "е");
    let mut text = text
        .trim_matches(|c: char| c.is_whitespace() || ".!?".contains(c))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(t) = text.strip_prefix("алиса") {
        if t.starts_with([',', ' ']) {
            text = t.trim_start_matches([',', ' ']).into();
        }
    }
    if let Some(t) = text.strip_suffix("пожалуйста") {
        text = t.trim_end_matches([',', ' ']).into();
    }
    if text.is_empty()
        || text.chars().count() > 500
        || text.split_whitespace().any(|w| w.starts_with("навык"))
    {
        return None;
    }
    let groups: &[(&str, &[&str])] = &[
        (
            "next",
            &[
                "дальше",
                "следующая",
                "следующая песня",
                "следующий трек",
                "музыку дальше",
            ],
        ),
        (
            "previous",
            &[
                "назад",
                "предыдущая",
                "предыдущая песня",
                "предыдущий трек",
                "музыку назад",
            ],
        ),
        (
            "pause",
            &[
                "пауза",
                "стоп",
                "хватит",
                "выключи музыку",
                "останови музыку",
                "поставь на паузу",
                "поставь музыку на паузу",
            ],
        ),
        (
            "resume",
            &[
                "играй",
                "продолжай",
                "продолжи",
                "продолжи музыку",
                "продолжай музыку",
            ],
        ),
        (
            "similar",
            &["включи похожие", "похожие песни", "включи похожую музыку"],
        ),
        (
            "announce_now",
            &[
                "что играет",
                "что сейчас играет",
                "что за песня",
                "что за песня сейчас играет",
                "какая песня играет",
                "какая песня сейчас играет",
                "что за трек",
                "что за трек сейчас играет",
                "какая музыка играет",
                "кто поет",
            ],
        ),
    ];
    for (action, phrases) in groups {
        if phrases.contains(&text.as_str()) {
            return recent.then(|| (action.to_string(), String::new()));
        }
    }
    let mut query = None;
    for prefix in ["поставь ", "включи ", "играй ", "сыграй "] {
        if let Some(q) = text.strip_prefix(prefix) {
            query = Some(q.to_string());
            break;
        }
    }
    let mut q = query?;
    for suffix in ["эпл мьюзик", "эпл мюзик", "apple music"] {
        for preposition in ["в", "на", "из"] {
            if let Some(t) = q.strip_suffix(&format!(" {preposition} {suffix}")) {
                q = t.into();
            }
        }
    }
    if [
        "музыку",
        "музыка",
        "мою музыку",
        "мои песни",
        "медиатеку",
        "любимую музыку",
        "любимые песни",
    ]
    .contains(&q.as_str())
    {
        return Some(("library".into(), String::new()));
    }
    for (prefix, action) in [
        ("альбом ", "album"),
        ("плейлист ", "playlist"),
        ("подборку ", "playlist"),
        ("исполнителя ", "artist"),
        ("песни исполнителя ", "artist"),
        ("песню ", "play"),
        ("трек ", "play"),
        ("композицию ", "play"),
        ("музыку ", "auto"),
    ] {
        if let Some(t) = q.strip_prefix(prefix) {
            return (!t.trim().is_empty()).then(|| (action.into(), t.trim().into()));
        }
    }
    for w in q.split_whitespace() {
        if [
            "свет",
            "ламп",
            "люстр",
            "телевизор",
            "розетк",
            "пылесос",
            "чайник",
            "таймер",
            "будильник",
            "напомин",
            "кондиционер",
            "радио",
            "новости",
            "сказк",
            "блютуз",
            "bluetooth",
            "громкость",
            "тише",
            "громче",
        ]
        .iter()
        .any(|p| w.starts_with(p))
        {
            return None;
        }
    }
    Some(("auto".into(), q))
}
struct Routing {
    last_text: String,
    last_at: Instant,
    active: Option<Instant>,
    last_routed: Option<((String, String), bool, Instant)>,
}
#[derive(Clone)]
struct Voice {
    shared: Shared,
    routing: Arc<Mutex<Routing>>,
}
impl Voice {
    async fn accept(&self, state: u64, text: &str, spotter: bool) {
        let mut r = self.routing.lock().await;
        if state == 1 {
            r.last_text.clear();
        }
        if state != 3 || text.is_empty() {
            return;
        }
        let now = Instant::now();
        if text == r.last_text && r.last_at.elapsed() < Duration::from_secs(3) {
            return;
        }
        r.last_text = text.into();
        r.last_at = now;
        let Some(candidate) = parse(text, true) else {
            return;
        };
        let view = self.shared.view.read().await;
        if view.airplay {
            return;
        }
        let native = self.shared.station.snapshot();
        let own = native["playerState"]["id"]
            .as_str()
            .is_some_and(|s| s.starts_with("alice-music-rust:"));
        let recent = r
            .active
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3600))
            || own
            || ((spotter || candidate.0 == "announce_now") && view.queue.current.is_some());
        drop(view);
        let Some(cmd) = parse(text, recent) else {
            return;
        };
        if r.last_routed.as_ref().is_some_and(|(c, source, t)| {
            c == &cmd && *source != spotter && t.elapsed() < Duration::from_secs(2)
        }) {
            return;
        }
        let Ok(permit) = self.shared.tx.try_reserve() else {
            return;
        };
        // Cancel the native interpretation before handing music commands to our queue.
        if wire::cancel_dialog(self.shared.config.recognition_port)
            .await
            .is_err()
        {
            return;
        }
        r.last_routed = Some((cmd.clone(), spotter, now));
        r.active = Some(now);
        println!("voice action={}", cmd.0);
        permit.send(Job {
            action: cmd.0,
            query: cmd.1,
            voice: true,
            reply: None,
            at: now,
        });
    }
    async fn recognition(self) {
        loop {
            let result: anyhow::Result<()> = async {
                let mut socket = tokio::net::TcpStream::connect((
                    "127.0.0.1",
                    self.shared.config.recognition_port,
                ))
                .await?;
                loop {
                    let frame = wire::read_frame(&mut socket).await?;
                    if let Ok(Some((state, text))) = wire::recognition(&frame) {
                        self.accept(state, &text, false).await;
                    }
                }
            }
            .await;
            if result.is_err() {
                eprintln!("recognition_reconnect");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
    async fn spotter(self, path: String) {
        use std::os::unix::fs::MetadataExt;
        let mut reader: Option<tokio::io::BufReader<tokio::fs::File>> = None;
        let mut identity = (0, 0);
        let mut first = true;
        let mut offset = 0;
        loop {
            if let Ok(meta) = tokio::fs::metadata(&path).await {
                let current = (meta.dev(), meta.ino());
                if reader.is_none() || current != identity {
                    if let Ok(mut file) = tokio::fs::File::open(&path).await {
                        offset = if first { meta.len() } else { 0 };
                        let _ = file.seek(std::io::SeekFrom::Start(offset)).await;
                        reader = Some(tokio::io::BufReader::new(file));
                        identity = current;
                        first = false;
                    }
                } else if meta.len() < offset {
                    if let Some(r) = &mut reader {
                        let _ = r.seek(std::io::SeekFrom::Start(0)).await;
                        offset = 0;
                    }
                }
                if let Some(r) = &mut reader {
                    for _ in 0..100 {
                        // Limit each read to avoid unbounded log lines consuming memory.
                        let mut bytes = vec![];
                        let read = tokio::io::AsyncReadExt::take(&mut *r, 8192)
                            .read_until(b'\n', &mut bytes)
                            .await;
                        let Ok(n) = read else {
                            reader = None;
                            break;
                        };
                        if n == 0 {
                            break;
                        }
                        offset += n as u64;
                        let line = String::from_utf8_lossy(&bytes);
                        if let Some((_, word)) = line.split_once("Command phrase spotted: ") {
                            let word = word.trim().trim_start_matches('^').trim().to_lowercase();
                            if [
                                "дальше",
                                "назад",
                                "пауза",
                                "стоп",
                                "хватит",
                                "играй",
                                "продолжай",
                            ]
                            .contains(&word.as_str())
                            {
                                self.accept(3, &word, true).await;
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
pub fn start(shared: Shared) {
    let voice = Voice {
        shared,
        routing: Arc::new(Mutex::new(Routing {
            last_text: String::new(),
            last_at: Instant::now(),
            active: None,
            last_routed: None,
        })),
    };
    tokio::spawn(voice.clone().recognition());
    tokio::spawn(voice.spotter("/data/quasar/daemons_logs/maind.log".into()));
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_other_alice_features() {
        for p in [
            "включи свет",
            "включи радио",
            "поставь таймер на пять минут",
            "запусти навык моя фонотека",
            "включи громкость три",
        ] {
            assert!(parse(p, true).is_none(), "{p}");
        }
    }
    #[test]
    fn ownership_required_for_controls() {
        assert!(parse("дальше", false).is_none());
        assert_eq!(
            parse("Алиса, дальше!", true),
            Some(("next".into(), "".into()))
        );
    }
    #[test]
    fn named_search_and_library() {
        assert_eq!(
            parse("Алиса, включи песню Babydoll в Apple Music", false),
            Some(("play".into(), "babydoll".into()))
        );
        assert_eq!(
            parse("включи мою музыку", false),
            Some(("library".into(), "".into()))
        );
    }
}
