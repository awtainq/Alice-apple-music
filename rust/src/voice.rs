use crate::{
    gemini::{native_followup, Assistant},
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
        .trim_matches(|c: char| c.is_whitespace() || ".!?,:;".contains(c))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(t) = text.strip_prefix("алиса") {
        if t.starts_with([',', ' ', ':', '!', '?', '.']) {
            text = t.trim_start_matches([',', ' ', ':', '!', '?', '.']).into();
        }
    }
    if let Some(t) = text.strip_prefix("пожалуйста") {
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
                "следующую",
                "следующий",
                "следующую песню",
                "включи следующую песню",
                "включи следующий трек",
                "переключи песню",
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
                "предыдущую",
                "предыдущую песню",
                "включи предыдущую песню",
                "включи предыдущий трек",
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
                "останови",
                "останови воспроизведение",
                "приостанови музыку",
                "поставь воспроизведение на паузу",
            ],
        ),
        (
            "resume",
            &[
                "играй",
                "продолжай",
                "продолжи",
                "продолжить",
                "продолжим",
                "возобнови",
                "продолжи музыку",
                "продолжай музыку",
                "играй музыку",
                "играй дальше",
                "продолжи играть",
                "продолжай играть",
                "продолжить музыку",
                "продолжи воспроизведение",
                "продолжай воспроизведение",
                "возобнови музыку",
                "возобнови воспроизведение",
                "сними с паузы",
                "сними музыку с паузы",
                "сними трек с паузы",
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
fn parse_for_queue(text: &str, has_current: bool, recent: bool) -> Option<(String, String)> {
    let mut command = parse(text, recent)?;
    if command.0 == "resume" && !has_current {
        command.0 = "library".into();
    }
    if has_current && command.0 == "library" {
        let normalized = crate::gemini::normalize(text);
        if ["включи музыку", "поставь музыку", "сыграй музыку"].contains(&normalized.as_str())
        {
            command.0 = "resume".into();
        }
    }
    Some(command)
}
fn spotter_command(text: &str) -> bool {
    parse(text, true).is_some_and(|(action, _)| {
        matches!(
            action.as_str(),
            "next" | "previous" | "pause" | "resume" | "similar" | "announce_now"
        )
    })
}
#[derive(Default)]
struct RequestIds(std::collections::VecDeque<String>);
impl RequestIds {
    fn seen(&mut self, id: Option<&str>) -> bool {
        let Some(id) = id.filter(|id| !id.is_empty()) else {
            return false;
        };
        if self.0.iter().any(|old| old == id) {
            return true;
        }
        self.0.push_back(id.into());
        if self.0.len() > 32 {
            self.0.pop_front();
        }
        false
    }
}
struct Routing {
    requests: RequestIds,
    last_text: String,
    last_at: Instant,
    active: Option<Instant>,
    last_routed: Option<((String, String), bool, Instant)>,
    native_until: Option<Instant>,
}
#[derive(Clone)]
struct Voice {
    shared: Shared,
    routing: Arc<Mutex<Routing>>,
    assistant: Option<Assistant>,
}
impl Voice {
    async fn accept(&self, state: u64, text: &str, spotter: bool, request_id: Option<&str>) {
        let mut r = self.routing.lock().await;
        if state == 1 {
            r.last_text.clear();
            if let Some(assistant) = &self.assistant {
                assistant.interrupt().await;
            }
        }
        if state != 3 || text.is_empty() {
            return;
        }
        if !spotter && r.requests.seen(request_id) {
            return;
        }
        let now = Instant::now();
        if text == r.last_text && r.last_at.elapsed() < Duration::from_secs(3) {
            return;
        }
        r.last_text = text.into();
        r.last_at = now;
        let candidate = parse(text, true);
        if let Some(assistant) = &self.assistant {
            let explicit_music = candidate
                .as_ref()
                .is_some_and(|(action, _)| action != "auto");
            if !spotter
                && !explicit_music
                && (assistant.native(text)
                    || r.native_until.is_some_and(|until| until > now) && native_followup(text))
            {
                assistant.interrupt().await;
                r.native_until = Some(now + Duration::from_secs(30));
                println!("assistant_route=alice");
                return;
            }
        }
        let Some(candidate) = candidate else {
            if !spotter {
                if let Some(assistant) = &self.assistant {
                    if wire::cancel_dialog(self.shared.config.recognition_port)
                        .await
                        .is_ok()
                    {
                        r.native_until = None;
                        println!("assistant_route=gemini");
                        assistant.submit(text).await;
                    }
                }
            }
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
        let has_current = view.queue.current.is_some();
        let recent = r
            .active
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3600))
            || own
            || has_current
            || candidate.0 == "resume";
        drop(view);
        let Some(cmd) = parse_for_queue(text, has_current, recent) else {
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
        if let Some(assistant) = &self.assistant {
            assistant.interrupt().await;
        }
        r.native_until = None;
        r.last_routed = Some((cmd.clone(), spotter, now));
        r.active = Some(now);
        println!(
            "voice action={} source={}",
            cmd.0,
            if spotter { "spotter" } else { "recognition" }
        );
        let revision = self.shared.begin_intent(&cmd.0);
        permit.send(Job {
            action: cmd.0,
            query: cmd.1,
            voice: true,
            reply: None,
            at: now,
            revision,
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
                let mut initial = true;
                loop {
                    let frame = wire::read_frame(&mut socket).await?;
                    if let Ok(Some((state, text))) = wire::recognition(&frame) {
                        let body = wire::get_bytes(&frame, 252)?.unwrap_or_default();
                        let id =
                            wire::get_bytes(&body, 7)?.and_then(|id| String::from_utf8(id).ok());
                        // A reconnect starts with the previous dialog snapshot.
                        if initial {
                            initial = false;
                            if !text.is_empty() {
                                self.routing.lock().await.requests.seen(id.as_deref());
                            }
                            continue;
                        }
                        self.accept(state, &text, false, id.as_deref()).await;
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
                            if spotter_command(&word) {
                                self.accept(3, &word, true, None).await;
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
pub fn start(shared: Shared, assistant: Option<Assistant>) {
    let voice = Voice {
        shared,
        assistant,
        routing: Arc::new(Mutex::new(Routing {
            requests: RequestIds::default(),
            last_text: String::new(),
            last_at: Instant::now(),
            active: None,
            last_routed: None,
            native_until: None,
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

#[cfg(test)]
mod resume_tests {
    use super::*;
    #[test]
    fn all_resume_forms_route_to_current_queue() {
        for phrase in [
            "продолжи",
            "продолжай",
            "продолжить",
            "продолжим",
            "возобнови",
            "играй",
        ] {
            assert!(spotter_command(phrase), "bare control missing: {phrase}");
            assert_eq!(
                parse(phrase, true),
                Some(("resume".into(), String::new())),
                "{phrase}"
            );
        }
    }
}

#[cfg(test)]
mod command_matrix_tests {
    use super::*;
    #[test]
    fn saved_queue_controls_with_and_without_wake_word() {
        let groups: &[(&str, &[&str])] = &[
            (
                "resume",
                &[
                    "играй",
                    "играй музыку",
                    "продолжи",
                    "продолжай",
                    "продолжить",
                    "возобнови музыку",
                    "продолжи воспроизведение",
                    "сними с паузы",
                ],
            ),
            (
                "pause",
                &[
                    "пауза",
                    "стоп",
                    "останови музыку",
                    "поставь музыку на паузу",
                ],
            ),
            (
                "next",
                &["дальше", "следующая песня", "включи следующий трек"],
            ),
            (
                "previous",
                &["назад", "предыдущая песня", "включи предыдущий трек"],
            ),
        ];
        for (action, phrases) in groups {
            for phrase in *phrases {
                for prefix in ["", "Алиса, ", "алиса ", "Алиса! ", "Алиса, пожалуйста, "]
                {
                    for suffix in ["", "!", ", пожалуйста"] {
                        let text = format!("{prefix}{phrase}{suffix}");
                        assert_eq!(
                            parse_for_queue(&text, true, true).unwrap().0,
                            *action,
                            "{text}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn first_play_uses_apple_library() {
        assert_eq!(
            parse_for_queue("Алиса, играй", false, true).unwrap().0,
            "library"
        );
    }
    #[test]
    fn generic_music_continues_saved_queue() {
        assert_eq!(
            parse_for_queue("Алиса, включи музыку", true, true)
                .unwrap()
                .0,
            "resume"
        );
        assert_eq!(
            parse_for_queue("Алиса, включи мою музыку", true, true)
                .unwrap()
                .0,
            "library"
        );
    }
}

#[cfg(test)]
mod request_replay_tests {
    use super::*;
    #[test]
    fn one_request_executes_once_even_after_another_request() {
        let mut ids = RequestIds::default();
        assert!(!ids.seen(Some("first")));
        assert!(ids.seen(Some("first")));
        assert!(!ids.seen(Some("second")));
        assert!(ids.seen(Some("first")));
        assert!(!ids.seen(None));
        assert!(!ids.seen(Some("")));
    }
}
