use crate::player::Shared;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use subtle::ConstantTimeEq;
use tower::ServiceExt;
use tower_http::services::ServeFile;

async fn auth(State(s): State<Shared>, request: Request<Body>, next: Next) -> Response {
    let expected = format!("Bearer {}", s.config.agent_token);
    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .map(|h| h.as_bytes())
        .unwrap_or_default();
    if !bool::from(expected.as_bytes().ct_eq(provided)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}
async fn status(State(s): State<Shared>) -> Json<serde_json::Value> {
    Json(s.status().await)
}
#[derive(Deserialize)]
struct Command {
    action: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    voice: bool,
}
async fn command(State(s): State<Shared>, Json(c): Json<Command>) -> Response {
    if c.action.len() > 32 || c.query.len() > 2000 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match s.command(&c.action, &c.query, c.voice).await {
        Ok(message) => Json(json!({"message":message,"status":s.status().await})).into_response(),
        Err(_) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error":"Не удалось выполнить команду."})),
        )
            .into_response(),
    }
}
async fn audio(
    State(s): State<Shared>,
    Path((token, name)): Path<(String, String)>,
    req: Request<Body>,
) -> Response {
    let Some(id) = name.strip_suffix(".m4a") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if id.is_empty() || !id.bytes().all(|c| c.is_ascii_digit()) || !s.verify(id, &token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let r = ServeFile::new(s.cache.join(name)).oneshot(req).await;
    match r {
        Ok(r) => {
            if r.status().is_success() {
                s.view.write().await.fetch_count += 1;
            }
            let mut r = r.into_response();
            r.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("audio/mp4"),
            );
            r
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
pub fn router(s: Shared) -> Router {
    let api = Router::new()
        .route("/api", get(status).post(command))
        .layer(middleware::from_fn_with_state(s.clone(), auth));
    Router::new()
        .merge(api)
        .route(
            "/healthz",
            get(|| async { Json(json!({"ok":true,"native":true,"implementation":"rust"})) }),
        )
        .route("/audio/:token/:name", get(audio))
        .layer(DefaultBodyLimit::max(8192))
        .with_state(s)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, player::View, station::Station};
    use std::sync::Arc;
    use tokio::sync::{mpsc, RwLock};
    fn shared(cache: std::path::PathBuf) -> Shared {
        let config:Config=serde_json::from_value(json!({"apple_app_token":"fixture","apple_user_token":"fixture","yandex_music_token":"fixture","yandex_x_token":"fixture","station_device_id":"fixture","station_platform":"fixture","agent_token":"fixture-only"})).unwrap();
        let (tx, _) = mpsc::channel(1);
        Shared {
            view: Arc::new(RwLock::new(View::default())),
            station: Station::fixture(),
            config: Arc::new(config),
            secret: [42; 32],
            cache,
            tx,
        }
    }
    #[tokio::test]
    async fn api_rejects_missing_or_wrong_token() {
        let app = router(shared(std::env::temp_dir()));
        for token in ["", "Bearer wrong"] {
            let r = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api")
                        .header("Authorization", token)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        }
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/api")
                    .header("Authorization", "Bearer fixture-only")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
    #[tokio::test]
    async fn audio_range_and_capability_auth() {
        let path = std::env::temp_dir().join(format!("alice-rust-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&path).await.unwrap();
        tokio::fs::write(path.join("12.m4a"), b"0123456789")
            .await
            .unwrap();
        let s = shared(path.clone());
        assert!(!s.verify("13", &s.token("12")));
        let uri = format!("/audio/{}/12.m4a", s.token("12"));
        let app = router(s);
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("Range", "bytes=2-5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(r.headers()["content-range"], "bytes 2-5/10");
        assert_eq!(
            axum::body::to_bytes(r.into_body(), 20).await.unwrap(),
            &b"2345"[..]
        );
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/audio/wrong/12.m4a")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        tokio::fs::remove_dir_all(path).await.unwrap();
    }
}
