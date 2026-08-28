use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ItemStatus {
    Playable,
    Pending,
    Invalid,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PlaybackMode {
    Ordered,
    ListLoop,
    SingleLoop,
    Random,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistItem {
    pub id: String,
    pub title: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub status: ItemStatus,
    pub position: u32,
    #[serde(default)]
    pub last_position_seconds: f64,
    #[serde(default)]
    pub play_count: u64,
    pub last_played_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedItem {
    pub id: String,
    pub title: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub status: ItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackContext {
    pub mode: PlaybackMode,
    pub current_item_id: Option<String>,
    pub current_position_seconds: f64,
    pub random_seed: Option<u64>,
    #[serde(default)]
    pub random_round: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistDocument {
    pub version: u32,
    pub updated_at: String,
    pub active_playlist_id: Option<String>,
    /// 全局播放模式（跨列表共享）。旧文件无此字段时 serde default 取 ListLoop。
    #[serde(default = "default_playback_mode")]
    pub playback_mode: PlaybackMode,
    pub playlists: Vec<LocalPlaylist>,
}

fn default_playback_mode() -> PlaybackMode {
    PlaybackMode::ListLoop
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalPlaylist {
    pub id: String,
    pub name: String,
    pub source_url: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub items: Vec<PlaylistItem>,
    pub playback: PlaybackContext,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditEvent {
    #[serde(default)]
    pub event_id: Option<String>,
    pub timestamp: String,
    pub event_type: String,
    pub item_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_playlist_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<PlaylistDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackEvent {
    #[serde(default)]
    pub event_id: Option<String>,
    pub timestamp: String,
    pub event_type: String,
    pub item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_playlist_url: Option<String>,
    #[serde(default)]
    pub position_seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PlaybackCommandDto {
    Load {
        url: String,
        #[serde(rename = "positionSeconds", alias = "position_seconds")]
        position_seconds: f64,
    },
    Play,
    Pause,
    Next,
    Previous,
    Seek {
        #[serde(rename = "positionSeconds", alias = "position_seconds")]
        position_seconds: f64,
    },
}
