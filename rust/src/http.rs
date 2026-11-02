use crate::player::Shared;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use subtle::ConstantTimeEq;

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
        .layer(DefaultBodyLimit::max(8192))
        .with_state(s)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, player::View, volume::Volume};
    use std::sync::Arc;
    use tokio::sync::{mpsc, RwLock};
    use tower::ServiceExt;
    fn shared() -> Shared {
        let config:Config=serde_json::from_value(json!({"apple_app_token":"fixture","apple_user_token":"fixture","yandex_music_token":"fixture","yandex_x_token":"fixture","station_device_id":"fixture","station_platform":"fixture","agent_token":"fixture-only"})).unwrap();
        let (tx, _) = mpsc::channel(1);
        Shared {
            view: Arc::new(RwLock::new(View::default())),
            volume: Volume::fixture(),
            config: Arc::new(config),
            intent: tokio::sync::watch::channel(0).0,
            speech: tokio::sync::watch::channel(u64::MAX).0,
            duck: tokio::sync::watch::channel(None).0,
            tx,
        }
    }
    #[tokio::test]
    async fn api_rejects_missing_or_wrong_token() {
        let app = router(shared());
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
}
