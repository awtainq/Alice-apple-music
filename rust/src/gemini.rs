use crate::{config::GeminiOptions, player::Shared};
use anyhow::{anyhow, bail, ensure, Result};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
pub const MODEL: &str = "gemini-3.5-flash-lite";
pub fn normalize(text: &str) -> String {
    let text = text.to_lowercase().replace('ё', "е");
    let text = text.trim_matches(|c: char| c.is_whitespace() || ".!?,".contains(c));
    let text = text
        .strip_prefix("алиса,")
        .or_else(|| text.strip_prefix("алиса "))
        .unwrap_or(text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn stem(word: &str) -> String {
    let trimmed = word.trim_end_matches(['а', 'я', 'ы', 'и', 'ь', 'ю', 'у', 'е', 'о', 'й']);
    if trimmed.chars().count() >= 3 {
        trimmed.into()
    } else {
        word.into()
    }
}
fn named_device(text: &str, names: &[String]) -> bool {
    let words: Vec<_> = text.split_whitespace().map(stem).collect();
    names.iter().any(|name| {
        let name = normalize(name);
        let parts: Vec<_> = name.split_whitespace().map(stem).collect();
        !parts.is_empty() && words.windows(parts.len()).any(|window| window == parts)
    })
}
pub fn native_topic(text: &str, names: &[String]) -> bool {
    let text = normalize(text);
    if names.iter().any(|name| normalize(name) == text) {
        return true;
    }
    if text.is_empty() || text == "алиса" {
        return true;
    }
    let words: Vec<_> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    let native_stems = [
        "погод",
        "прогноз",
        "дожд",
        "снег",
        "ветер",
        "осадк",
        "рассвет",
        "закат",
        "будильник",
        "таймер",
        "напоминан",
        "громкост",
        "блютуз",
        "bluetooth",
        "навык",
    ];
    if words
        .iter()
        .any(|word| native_stems.iter().any(|s| word.starts_with(s)))
    {
        return true;
    }
    if [
        "температура на улице",
        "температура дома",
        "как на улице",
        "восход солнца",
        "атмосферное давление",
        "умный дом",
        "умного дома",
        "умном доме",
        "который час",
        "сколько времени",
        "яндекс музыку",
        "яндекс музыка",
        "яндекс музыки",
    ]
    .iter()
    .any(|p| text.contains(p))
    {
        return true;
    }
    if [
        "сколько градусов",
        "какая температура",
        "тише",
        "громче",
        "отмена",
        "отбой",
        "не надо",
        "выключись",
        "что ты умеешь",
        "помощь",
    ]
    .contains(&text.as_str())
    {
        return true;
    }
    let control = [
        "включи",
        "выключи",
        "погаси",
        "поставь",
        "установи",
        "сделай",
        "открой",
        "закрой",
        "запусти",
        "останови",
        "убавь",
        "прибавь",
    ]
    .iter()
    .any(|p| text.starts_with(p));
    let state = [
        "включен",
        "выключен",
        "включа",
        "выключа",
        "открыт",
        "закрыт",
        "заряд",
        "состояни",
        "статус",
        "дома",
        "в доме",
        "в комнат",
        "в спальн",
        "на кухн",
        "яркост",
        "температур",
        "влажност",
    ]
    .iter()
    .any(|p| text.contains(p));
    let home = [
        "свет",
        "ламп",
        "люстр",
        "розет",
        "выключател",
        "термостат",
        "кондиционер",
        "обогревател",
        "вентилятор",
        "пылесос",
        "штор",
        "жалюзи",
        "замок",
        "двер",
        "датчик",
        "чайник",
        "увлажнител",
        "сценар",
    ];
    (control || state)
        && (named_device(&text, names)
            || words
                .iter()
                .any(|word| home.iter().any(|s| word.starts_with(s))))
}
pub fn native_followup(text: &str) -> bool {
    let text = normalize(text);
    let words: Vec<_> = text.split_whitespace().collect();
    if words.len() > 5 {
        return false;
    }
    matches!(
        text.as_str(),
        "да" | "нет"
            | "везде"
            | "все"
            | "включи"
            | "выключи"
            | "завтра"
            | "сегодня"
            | "послезавтра"
            | "на выходных"
            | "через час"
    ) || (matches!(words.first(), Some(&"в") | Some(&"на"))
        && !words
            .get(1)
            .is_some_and(|w| ["чем", "чего", "каком", "какой", "сколько"].contains(w)))
        || text.chars().all(|c| c.is_ascii_digit() || c == ' ')
}
fn response_text(data: &Value) -> Result<String> {
    let raw = data["candidates"][0]["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["thought"] != true)
        .filter_map(|p| p["text"].as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let tags = regex::Regex::new(r"<[^>]{0,200}>")?;
    let plain = tags.replace_all(&raw, "");
    let plain = plain.replace(['*', '`', '#'], "");
    let text = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(!text.is_empty(), "Gemini returned no answer");
    Ok(text.chars().take(700).collect())
}
async fn generate(
    http: &reqwest::Client,
    key: &str,
    mut contents: Vec<Value>,
    text: &str,
) -> Result<String> {
    contents.push(json!({"role":"user","parts":[{"text":text}]}));
    let body = json!({"systemInstruction":{"parts":[{"text":format!("Ты Google Gemini, модель {MODEL}. Алиса — голосовой интерфейс. Отвечай по-русски, ясно и кратко, обычно до 700 символов. Без Markdown и SSML. Не представляйся ЯндексGPT. Ты не управляешь устройствами. Если актуальные данные неизвестны, не выдумывай их.")}]},"contents":contents,"generationConfig":{"maxOutputTokens":1024,"thinkingConfig":{"thinkingLevel":"MINIMAL"}}});
    let mut response = http
        .post(format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{MODEL}:generateContent"
        ))
        .header("x-goog-api-key", key)
        .json(&body)
        .send()
        .await
        .map_err(|_| anyhow!("Gemini connection failed"))?;
    if !response.status().is_success() {
        bail!("Gemini HTTP {}", response.status().as_u16());
    }
    let mut bytes = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Gemini response failed"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= 128 * 1024,
            "Gemini response too large"
        );
        bytes.extend(chunk);
    }
    response_text(&serde_json::from_slice::<Value>(&bytes)?)
}
pub async fn probe(private: &Path, text: &str) -> Result<String> {
    let key = tokio::fs::read_to_string(private.join("gemini-api-key.txt"))
        .await
        .map_err(|_| anyhow!("Gemini key file is missing"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(40))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    generate(&client, key.trim(), vec![], text).await
}
struct Inner {
    http: reqwest::Client,
    key: String,
    shared: Shared,
    history: Mutex<Vec<Value>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    generation: AtomicU64,
    pub names: Vec<String>,
}
#[derive(Clone)]
pub struct Assistant(Arc<Inner>);
impl Assistant {
    pub async fn new(
        options: &GeminiOptions,
        private: &Path,
        shared: Shared,
    ) -> Result<Option<Self>> {
        if !options.enabled {
            return Ok(None);
        }
        let key = tokio::fs::read_to_string(private.join("gemini-api-key.txt"))
            .await
            .map_err(|_| anyhow!("Gemini key file is missing"))?;
        ensure!(!key.trim().is_empty(), "Gemini key is empty");
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(40))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        println!("gemini_ready model={MODEL}");
        Ok(Some(Self(Arc::new(Inner {
            http,
            key: key.trim().into(),
            shared,
            history: Mutex::new(vec![]),
            task: Mutex::new(None),
            generation: AtomicU64::new(0),
            names: options.alice_device_names.clone(),
        }))))
    }
    pub fn native(&self, text: &str) -> bool {
        native_topic(text, &self.0.names)
    }
    pub async fn interrupt(&self) {
        self.0.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(task) = self.0.task.lock().await.take() {
            task.abort();
        }
    }
    async fn answer(&self, text: &str) -> Result<String> {
        generate(
            &self.0.http,
            &self.0.key,
            self.0.history.lock().await.clone(),
            text,
        )
        .await
    }
    pub async fn submit(&self, text: &str) {
        self.interrupt().await;
        let generation = self.0.generation.load(Ordering::SeqCst);
        let this = self.clone();
        let question = text.chars().take(1500).collect::<String>();
        let mut task = self.0.task.lock().await;
        *task = Some(tokio::spawn(async move {
            let started = Instant::now();
            let result = this.answer(&question).await;
            if this.0.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            let answer = match result {
                Ok(answer) => {
                    let mut history = this.0.history.lock().await;
                    history.push(json!({"role":"user","parts":[{"text":question}]}));
                    history.push(json!({"role":"model","parts":[{"text":answer}]}));
                    if history.len() > 8 {
                        let n = history.len() - 8;
                        history.drain(..n);
                    }
                    println!(
                        "gemini_answered seconds={:.2}",
                        started.elapsed().as_secs_f64()
                    );
                    answer
                }
                Err(_) => {
                    eprintln!("gemini_request_failed");
                    "Gemini сейчас недоступен. Попробуйте позже.".into()
                }
            };
            if this.0.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            let _ = this.0.shared.command("speak_text", &answer, true).await;
        }));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_weather_and_home_in_alice() {
        for p in [
            "Какая погода завтра?",
            "Будет ли дождь?",
            "Алиса, включи свет в спальне",
            "Выключи вентилятор",
            "Установи термостат на 22",
            "Поставь таймер на пять минут",
            "Сколько времени?",
        ] {
            assert!(native_topic(p, &[]), "{p}");
        }
    }
    #[test]
    fn knowledge_goes_to_gemini() {
        for p in [
            "Почему небо голубое?",
            "Сколько будет 17 плюс 25?",
            "Расскажи про Рим",
            "Температура кипения воды",
            "Придумай сказку",
            "Что такое скорость света?",
            "Расскажи про средневековый замок",
            "Ты ЯндексGPT?",
        ] {
            assert!(!native_topic(p, &[]), "{p}");
        }
    }
    #[test]
    fn protects_custom_device_names() {
        assert!(native_topic("Включи Люсю", &["Люся".into()]));
        assert!(!native_topic("Включи Люсю", &[]));
    }
    #[test]
    fn native_dialogue_followups() {
        assert!(native_followup("В Москве"));
        assert!(native_followup("Завтра"));
        assert!(!native_followup("Почему небо голубое?"));
    }
    #[test]
    fn strips_markup_and_thoughts() {
        assert_eq!(response_text(&json!({"candidates":[{"content":{"parts":[{"text":"secret thought","thought":true},{"text":"<speak>Ответ **42**.</speak>"}]}}]})).unwrap(),"Ответ 42.");
    }
}
