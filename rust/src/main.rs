mod airplay;
mod apple;
mod config;
mod http;
mod player;
mod station;
mod voice;
mod wire;
use anyhow::{ensure, Result};
use clap::Parser;
use std::{path::PathBuf, sync::Arc};
#[derive(Parser)]
#[command(
    version,
    about = "Local Apple Music player for rooted Yandex Station / Luna"
)]
struct Args {
    #[arg(long, default_value = "/data/opt/alice-music/private")]
    private: PathBuf,
    #[arg(long, default_value = "/tmp/alice-apple-music-cache")]
    cache: PathBuf,
    #[arg(long)]
    check: Option<String>,
    #[arg(long)]
    local_voice: bool,
    #[arg(long)]
    luna: bool,
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("player_exit: {}", error);
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let a = Args::parse();
    // Luna grants exec to this trusted native plugin. Validate runtime identity before opening local IPC.
    if a.luna {
        let url = std::env::var("LUNA_API_URL")?;
        let token = std::env::var("LUNA_API_TOKEN")?;
        ensure!(
            url.starts_with("http://127.0.0.1:"),
            "Luna runtime must be local"
        );
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let response = client
            .get(format!("{url}/device"))
            .header("x-luna-token", token)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Luna runtime unavailable"))?;
        ensure!(
            response.status().is_success(),
            "Luna runtime denied exec permission"
        );
    }
    let config = Arc::new(config::Config::load(&a.private)?);
    // Bind first so a duplicate instance cannot subscribe to voice or touch playback.
    let listener = if a.check.is_none() {
        Some(tokio::net::TcpListener::bind(("127.0.0.1", config.port)).await?)
    } else {
        None
    };
    let apple = apple::Apple::new(config.clone(), a.private.clone(), a.cache).await?;
    if let Some(q) = a.check {
        let tracks = apple.select(&q, "play").await?;
        let t = tracks
            .first()
            .ok_or_else(|| anyhow::anyhow!("Nothing found"))?;
        let path = apple.prepare(t).await?;
        println!(
            "{} — {} ({} bytes)",
            t.title,
            t.artist,
            tokio::fs::metadata(path).await?.len()
        );
        return Ok(());
    }
    let station = station::Station::start(config.clone(), apple.http.clone());
    let (player, rx) = player::Player::new(apple, station, a.private);
    let shared = player.shared.clone();
    let actor = tokio::spawn(player.run(rx));
    if let Some(bus) = &config.airplay_bus {
        tokio::spawn(airplay::run(shared.clone(), bus.clone()));
    }
    if a.local_voice {
        voice::start(shared.clone());
    }
    let shutdown_shared = shared.clone();
    let shutdown = async move {
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM");
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            shutdown_shared.command("pause", "", false),
        )
        .await;
    };
    println!(
        "ready port={} version={}",
        config.port,
        env!("CARGO_PKG_VERSION")
    );
    let result = axum::serve(listener.expect("service listener"), http::router(shared))
        .with_graceful_shutdown(shutdown)
        .await;
    actor.abort();
    result?;
    Ok(())
}
