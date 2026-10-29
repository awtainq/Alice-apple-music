use crate::player::Shared;
use std::time::Duration;
pub async fn run(s: Shared, path: String) {
    loop {
        let result: anyhow::Result<()> = async {
            let address = format!("unix:path={path}");
            let bus = zbus::connection::Builder::address(address.as_str())?
                .build()
                .await?;
            let proxy = zbus::Proxy::new(
                &bus,
                "org.gnome.ShairportSync",
                "/org/gnome/ShairportSync",
                "org.gnome.ShairportSync",
            )
            .await?;
            loop {
                let reply = tokio::time::timeout(
                    Duration::from_secs(2),
                    proxy.call::<_, _, (u32, bool, String)>("GetControlStatus", &()),
                )
                .await??;
                let active = reply.0 == 1 && reply.1 && !reply.2.is_empty();
                let mut view = s.view.write().await;
                if view.airplay != active {
                    println!("airplay_active={active}");
                }
                view.airplay = active;
                drop(view);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        .await;
        if result.is_err() {
            s.view.write().await.airplay = false;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
