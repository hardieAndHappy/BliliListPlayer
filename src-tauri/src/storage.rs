use crate::model::{EditEvent, PlaybackEvent, PlaylistDocument};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_EVENTS: usize = 10_000;

#[derive(Debug, Clone)]
pub struct Storage {
    pub data_dir: PathBuf,
}

impl Storage {
    pub fn new(runtime_dir: impl AsRef<Path>) -> io::Result<Self> {
        // Portable storage: keep the archive beside the executable's directory.
        // The caller supplies the desired runtime directory (normally exe_dir).
        let data_dir = runtime_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)?;
        Ok(Self { data_dir })
    }
    fn playlist_path(&self) -> PathBuf {
        self.data_dir.join("playlists.json")
    }
    fn backup_path(&self) -> PathBuf {
        self.data_dir.join("playlists.json.backup")
    }
    pub fn load_playlist(&self) -> io::Result<Option<PlaylistDocument>> {
        let path = self.playlist_path();
        if !path.exists() {
            return self.load_backup();
        }
        match fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(doc) => Ok(Some(doc)),
            None => {
                let corrupt = self
                    .data_dir
                    .join(format!("playlists.json.corrupt.{}", timestamp()));
                let _ = fs::rename(&path, corrupt);
                self.load_backup()
            }
        }
    }
    fn load_backup(&self) -> io::Result<Option<PlaylistDocument>> {
        let path = self.backup_path();
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)?;
        let document = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::copy(path, self.playlist_path())?;
        Ok(Some(document))
    }
    pub fn save_playlist(&self, document: &PlaylistDocument) -> io::Result<()> {
        let path = self.playlist_path();
        let temp = self
            .data_dir
            .join(format!("playlists.json.tmp.{}", timestamp()));
        let data = serde_json::to_vec_pretty(document).map_err(io::Error::other)?;
        let mut file = fs::File::create(&temp)?;
        file.write_all(&data)?;
        file.sync_all()?;
        if path.exists() {
            fs::copy(&path, self.backup_path())?;
        }
        if path.exists() {
            fs::remove_file(&path)?;
        }
        fs::rename(temp, path)?;
        Ok(())
    }
    pub fn append_edit_event(&self, event: &EditEvent) -> io::Result<()> {
        self.append_jsonl("edit-history.jsonl", event)
    }
    pub fn append_playback_event(&self, event: &PlaybackEvent) -> io::Result<()> {
        self.append_jsonl("playback-history.jsonl", event)
    }
    fn append_jsonl<T: serde::Serialize>(&self, name: &str, event: &T) -> io::Result<()> {
        let path = self.data_dir.join(name);
        let line = serde_json::to_string(event).map_err(io::Error::other)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "{}", line)?;
        file.sync_data()?;
        let lines = fs::read_to_string(&path)?
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if lines.len() > MAX_EVENTS {
            let retained = &lines[lines.len() - MAX_EVENTS..];
            let temp = self.data_dir.join(format!("{}.tmp.{}", name, timestamp()));
            fs::write(&temp, format!("{}\n", retained.join("\n")))?;
            fs::remove_file(&path)?;
            fs::rename(temp, path)?;
        }
        Ok(())
    }
}
fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    fn storage() -> Storage {
        let p = std::env::temp_dir().join(format!("bili-test-{}", timestamp()));
        Storage::new(p).unwrap()
    }

    #[test]
    fn storage_uses_runtime_directory_directly() {
        let p = std::env::temp_dir().join(format!("bili-test-direct-{}", timestamp()));
        let s = Storage::new(&p).unwrap();
        assert_eq!(s.data_dir, p);
    }
    fn doc() -> PlaylistDocument {
        PlaylistDocument {
            version: 1,
            updated_at: "now".into(),
            active_playlist_id: Some("local-1".into()),
            playback_mode: PlaybackMode::Ordered,
            playlists: vec![LocalPlaylist {
                id: "local-1".into(),
                name: "Temporary import".into(),
                source_url: "https://www.bilibili.com/list/1".into(),
                status: "active".into(),
                created_at: "now".into(),
                updated_at: "now".into(),
                items: vec![],
                playback: PlaybackContext {
                    mode: PlaybackMode::Ordered,
                    current_item_id: None,
                    current_position_seconds: 0.0,
                    random_seed: None,
                    random_round: vec![],
                },
            }],
        }
    }
    #[test]
    fn serializes_null_position() {
        let json = serde_json::to_string(&doc()).unwrap();
        assert!(json.contains("\"currentItemId\":null"));
        assert!(json.contains("\"activePlaylistId\":\"local-1\""));
        assert!(json.contains("\"sourceUrl\":\"https://www.bilibili.com/list/1\""));
    }
    #[test]
    fn saves_and_recovers_corrupt_file() {
        let s = storage();
        s.save_playlist(&doc()).unwrap();
        let mut changed = doc();
        changed.version = 2;
        s.save_playlist(&changed).unwrap();
        fs::write(s.playlist_path(), "bad").unwrap();
        assert_eq!(s.load_playlist().unwrap().unwrap().version, 1);
    }
    #[test]
    fn rotates_history() {
        let s = storage();
        for i in 0..(MAX_EVENTS + 2) {
            s.append_playback_event(&PlaybackEvent {
                event_id: None,
                timestamp: i.to_string(),
                event_type: "played".into(),
                item_id: None,
                playlist_id: None,
                source_playlist_url: None,
                position_seconds: 0.0,
                error: None,
            })
            .unwrap();
        }
        let text = fs::read_to_string(s.data_dir.join("playback-history.jsonl")).unwrap();
        assert_eq!(text.lines().count(), MAX_EVENTS);
    }
}
