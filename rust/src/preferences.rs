use crate::apple::Track;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Preferences {
    #[serde(default)]
    songs: BTreeMap<String, Rating>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Rating {
    track: Track,
    liked: bool,
}
impl Preferences {
    pub fn load(private: &Path) -> Result<Self> {
        match std::fs::read(private.join("preferences.json")) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn rate(&mut self, track: Track, liked: bool) {
        self.songs.insert(track.id.clone(), Rating { track, liked });
    }
    pub fn blocked(&self, id: &str) -> bool {
        self.songs.get(id).is_some_and(|r| !r.liked)
    }
    pub fn favorites(&self) -> Vec<Track> {
        self.songs
            .values()
            .filter(|r| r.liked)
            .map(|r| r.track.clone())
            .collect()
    }
    pub async fn save(&self, private: &Path) -> Result<()> {
        let temp = private.join("preferences.tmp");
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temp).await?;
        use tokio::io::AsyncWriteExt;
        file.write_all(&serde_json::to_vec(self)?).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(temp, private.join("preferences.json")).await?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> Track {
        Track {
            id: "123".into(),
            title: "Fixture".into(),
            artist: "Test".into(),
            duration: 10.,
        }
    }
    #[tokio::test]
    async fn ratings_survive_restart_and_can_be_reversed() {
        let dir = std::env::temp_dir().join(format!("apple-ratings-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&dir).await.unwrap();
        let mut p = Preferences::load(&dir).unwrap();
        p.rate(track(), true);
        p.rate(track(), true);
        p.save(&dir).await.unwrap();
        let mut p = Preferences::load(&dir).unwrap();
        assert_eq!(p.favorites(), vec![track()]);
        assert!(!p.blocked("123"));
        p.rate(track(), false);
        p.save(&dir).await.unwrap();
        let mut p = Preferences::load(&dir).unwrap();
        assert!(p.blocked("123"));
        assert!(p.favorites().is_empty());
        assert!(!p.blocked("999"));
        p.rate(track(), true);
        assert!(!p.blocked("123"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.join("preferences.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }
    #[test]
    fn corrupt_ratings_are_not_silently_replaced() {
        let dir = std::env::temp_dir().join(format!("apple-ratings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("preferences.json"), b"broken").unwrap();
        assert!(Preferences::load(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
