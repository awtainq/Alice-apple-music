//! A single final-text listener, following hackalice's Spotify recipe.
use crate::{
    gemini::{native_followup, Assistant},
    player::{Job, Shared},
    wire,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
fn clean(text: &str) -> String {
    let text = text.to_lowercase().replace('ё', "е");
    let text = text.trim_matches(|c: char| c.is_whitespace() || ".!?,:;".contains(c));
    let text = text
        .strip_prefix("алиса,")
        .or_else(|| text.strip_prefix("алиса "))
        .unwrap_or(text)
        .trim();
    text.strip_suffix(" пожалуйста")
        .unwrap_or(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn parse(text: &str, recent: bool) -> Option<(String, String)> {
    let text = clean(text);
    if text.is_empty() || text.chars().count() > 500 || text.contains("навык") {
        return None;
    }
    let transport = match text.as_str() {
        "мне нравится" | "мне нравится эта песня" | "нравится" | "лайк" | "добавь в избранное" => {
            Some("like")
        }
        "мне не нравится"
        | "мне не нравится эта песня"
        | "не нравится"
        | "дизлайк"
        | "больше не включай эту песню" => Some("dislike"),
        "пауза"
        | "стоп"
        | "хватит"
        | "выключи музыку"
        | "останови музыку"
        | "поставь на паузу"
        | "поставь музыку на паузу" => Some("pause"),
        "включи похожие" | "включи похожую музыку" => {
            Some("similar")
        }
        _ => None,
    };
    if let Some(action) = transport {
        return recent.then(|| (action.into(), String::new()));
    }
    if [
        "включи понравившиеся",
        "включи понравившиеся песни",
        "включи избранное",
    ]
    .contains(&text.as_str())
    {
        return Some(("favorites".into(), String::new()));
    }
    // Explicit search precedes transport words, so a song called "Дальше" is searchable.
    for prefix in ["поставь ", "включи ", "играй ", "сыграй "] {
        if let Some(query) = text.strip_prefix(prefix) {
            let mut q = query.to_string();
            for suffix in [
                " в apple music",
                " на apple music",
                " в эпл мьюзик",
                " в эпл мюзик",
            ] {
                if let Some(value) = q.strip_suffix(suffix) {
                    q = value.into();
                }
            }
            if ["следующую", "следующую песню", "следующий трек"].contains(&q.as_str())
            {
                return recent.then(|| ("next".into(), String::new()));
            }
            if ["предыдущую", "предыдущую песню", "предыдущий трек"].contains(&q.as_str())
            {
                return recent.then(|| ("previous".into(), String::new()));
            }
            if [
                "музыку",
                "музыка",
                "мою музыку",
                "мои песни",
                "медиатеку",
                "любимое",
                "любимую музыку",
                "любимые песни",
                "что-нибудь",
            ]
            .contains(&q.as_str())
            {
                return Some(("library".into(), String::new()));
            }
            for (p, action) in [
                ("песню ", "play"),
                ("трек ", "play"),
                ("композицию ", "play"),
                ("альбом ", "album"),
                ("плейлист ", "playlist"),
                ("подборку ", "playlist"),
                ("исполнителя ", "artist"),
            ] {
                if let Some(value) = q.strip_prefix(p) {
                    return (!value.trim().is_empty())
                        .then(|| (action.into(), value.trim().into()));
                }
            }
            // Native home/system requests are filtered by the router, before auto search.
            return (!q.is_empty()).then(|| ("auto".into(), q));
        }
    }
    let action = if [
        "пауза",
        "стоп",
        "хватит",
        "выключи музыку",
        "останови музыку",
        "поставь на паузу",
        "поставь музыку на паузу",
    ]
    .contains(&text.as_str())
    {
        "pause"
    } else if text.starts_with("продолж")
        || ["играй", "возобнови", "возобнови музыку", "сними с паузы"].contains(&text.as_str())
    {
        "resume"
    } else if text.starts_with("следующ") || text == "дальше" {
        "next"
    } else if text.starts_with("предыдущ") || text == "назад" {
        "previous"
    } else if [
        "что играет",
        "что сейчас играет",
        "что за песня",
        "что за песня сейчас играет",
        "какая песня играет",
        "кто поет",
    ]
    .contains(&text.as_str())
    {
        "announce_now"
    } else if ["включи похожие", "похожие песни", "включи похожую музыку"].contains(&text.as_str())
    {
        "similar"
    } else {
        return None;
    };
    recent.then(|| (action.into(), String::new()))
}
#[derive(Default)]
struct Requests(std::collections::VecDeque<String>);
impl Requests {
    fn duplicate(&mut self, id: Option<&str>) -> bool {
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
    requests: Requests,
    last_text: String,
    last_at: Instant,
    native_until: Option<Instant>,
}
#[derive(Clone)]
struct Voice {
    shared: Shared,
    assistant: Option<Assistant>,
    routing: Arc<Mutex<Routing>>,
}
impl Voice {
    async fn accept(&self, state: u64, text: &str, id: Option<&str>) {
        self.shared.speech.send_replace(state);
        if state == 1 {
            if let Some(assistant) = &self.assistant {
                assistant.interrupt().await;
            }
            self.shared
                .duck
                .send_replace(Some(Instant::now() + Duration::from_secs(15)));
        }
        if state == 0 {
            self.shared.duck.send_replace(None);
        }
        if state != 3 || text.is_empty() {
            return;
        }
        let mut routing = self.routing.lock().await;
        if routing.requests.duplicate(id) {
            return;
        }
        let normalized = clean(text);
        if normalized == routing.last_text && routing.last_at.elapsed() < Duration::from_secs(3) {
            return;
        }
        routing.last_text = normalized;
        routing.last_at = Instant::now();
        let view = self.shared.view.read().await;
        let recent = view.state == "playing"
            || view
                .last_started
                .is_some_and(|at| at.elapsed() < Duration::from_secs(3600));
        let airplay = view.airplay;
        drop(view);
        let candidate = parse(text, recent);
        if candidate.is_none() && parse(text, true).is_some() {
            return;
        }

        let explicit = candidate
            .as_ref()
            .is_some_and(|(action, _)| action != "auto");
        if let Some(assistant) = &self.assistant {
            if !explicit
                && (assistant.native(text)
                    || routing
                        .native_until
                        .is_some_and(|until| until > Instant::now())
                        && native_followup(text))
            {
                assistant.interrupt().await;
                routing.native_until = Some(Instant::now() + Duration::from_secs(30));
                return;
            }
        } else if !explicit && crate::gemini::native_topic(text, &[]) {
            return;
        }
        if let Some((action, query)) = candidate {
            if airplay {
                return;
            }
            if let Some(assistant) = &self.assistant {
                assistant.interrupt().await;
            }
            let Ok(permit) = self.shared.tx.try_reserve() else {
                return;
            };
            routing.native_until = None;
            // Claim only recognized music requests, before searching or speaking.
            // Native weather/home requests returned above remain untouched.
            if wire::cancel_dialog(self.shared.config.recognition_port)
                .await
                .is_err()
            {
                eprintln!("music_cancel_failed");
                return;
            }
            let revision = self.shared.begin_intent(&action);
            println!("voice action={action} source=final_text");
            permit.send(Job {
                action,
                query,
                voice: true,
                reply: None,
                at: routing.last_at,
                revision,
            });
        } else if let Some(assistant) = &self.assistant {
            // Non-native questions are delegated to Gemini.
            if wire::cancel_dialog(self.shared.config.recognition_port)
                .await
                .is_ok()
            {
                routing.native_until = None;
                assistant.submit(text).await;
            }
        }
    }
    async fn run(self) {
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
                    if let Some((state, text)) = wire::recognition(&frame)? {
                        let body = wire::get_bytes(&frame, 252)?.unwrap_or_default();
                        let id =
                            wire::get_bytes(&body, 7)?.and_then(|id| String::from_utf8(id).ok());
                        if initial {
                            initial = false;
                            self.shared.speech.send_replace(state);
                            if !text.is_empty() {
                                self.routing.lock().await.requests.duplicate(id.as_deref());
                            }
                            continue;
                        }
                        self.accept(state, &text, id.as_deref()).await;
                    }
                }
            }
            .await;
            if result.is_err() {
                self.shared.speech.send_replace(u64::MAX);
                self.shared.duck.send_replace(None);
                eprintln!("recognition_reconnect");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}
pub fn start(shared: Shared, assistant: Option<Assistant>) {
    tokio::spawn(
        Voice {
            shared,
            assistant,
            routing: Arc::new(Mutex::new(Routing {
                requests: Requests::default(),
                last_text: String::new(),
                last_at: Instant::now(),
                native_until: None,
            })),
        }
        .run(),
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recipe_commands_and_search() {
        for (prefix, action) in [
            ("песню ", "play"),
            ("альбом ", "album"),
            ("плейлист ", "playlist"),
            ("исполнителя ", "artist"),
            ("", "auto"),
        ] {
            assert_eq!(
                parse(&format!("Алиса, включи {prefix}Babydoll"), false),
                Some((action.into(), "babydoll".into()))
            );
        }
        assert_eq!(
            parse("включи песню Дальше", true),
            Some(("play".into(), "дальше".into()))
        );
        assert_eq!(parse("поставь мою музыку", false).unwrap().0, "library");
        assert_eq!(
            parse("Алиса, включи понравившиеся песни", false).unwrap().0,
            "favorites"
        );
        assert!(parse("мне нравится погода", true).is_none());
        assert_eq!(
            parse("включи песню Мне нравится", true).unwrap(),
            ("play".into(), "мне нравится".into())
        );
        for (text, action) in [
            ("мне нравится", "like"),
            ("мне не нравится", "dislike"),
            ("добавь в избранное", "like"),
            ("больше не включай эту песню", "dislike"),
            ("пауза", "pause"),
            ("продолжи", "resume"),
            ("дальше", "next"),
            ("назад", "previous"),
            ("что играет", "announce_now"),
        ] {
            assert_eq!(parse(text, true).unwrap().0, action);
            assert!(parse(text, false).is_none());
        }
    }
    #[test]
    fn request_is_only_handled_once() {
        let mut ids = Requests::default();
        assert!(!ids.duplicate(Some("one")));
        assert!(!ids.duplicate(Some("two")));
        assert!(ids.duplicate(Some("one")));
    }
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use crate::{config::Config, player::View, volume::Volume};
    use serde_json::json;
    use tokio::sync::{mpsc, watch, RwLock};
    fn fixture(port: u16) -> (Voice, mpsc::Receiver<Job>) {
        let config:Config=serde_json::from_value(json!({"apple_app_token":"fixture","apple_user_token":"fixture","agent_token":"fixture","recognition_port":port})).unwrap();
        let (tx, rx) = mpsc::channel(8);
        let shared = Shared {
            view: Arc::new(RwLock::new(View::default())),
            volume: Volume::fixture(),
            config: Arc::new(config),
            tx,
            intent: watch::channel(0).0,
            speech: watch::channel(u64::MAX).0,
            duck: watch::channel(None).0,
        };
        let voice = Voice {
            shared,
            assistant: None,
            routing: Arc::new(Mutex::new(Routing {
                requests: Requests::default(),
                last_text: String::new(),
                last_at: Instant::now(),
                native_until: None,
            })),
        };
        (voice, rx)
    }
    #[tokio::test]
    async fn music_cancels_native_dialog_before_dispatch() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (voice, mut rx) = fixture(port);
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut methods = vec![];
            for _ in 0..4 {
                let frame = wire::read_frame(&mut socket).await.unwrap();
                let remote = wire::get_bytes(&frame, 252).unwrap().unwrap();
                let request = wire::get_bytes(&remote, 8).unwrap().unwrap();
                methods.push(wire::get_number(&request, 1).unwrap().unwrap());
            }
            assert_eq!(methods, vec![1, 6, 14, 3]);
            assert!(rx.try_recv().is_err());
            rx
        });
        voice
            .accept(3, "включи песню Babydoll", Some("music-one"))
            .await;
        let mut rx = server.await.unwrap();
        let job = rx.try_recv().unwrap();
        assert_eq!(job.action, "play");
        assert_eq!(job.query, "babydoll");
        voice
            .accept(3, "включи песню Babydoll", Some("music-one"))
            .await;
        assert!(rx.try_recv().is_err());
    }
    #[tokio::test]
    async fn weather_does_not_cancel_native_dialog() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (voice, mut rx) = fixture(port);
        voice
            .accept(3, "какая погода завтра", Some("weather-one"))
            .await;
        assert!(rx.try_recv().is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(40), listener.accept())
                .await
                .is_err()
        );
    }
}
