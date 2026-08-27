export type ItemStatus = 'playable' | 'pending' | 'invalid' | 'deleted';
export type PlaybackMode = 'ordered' | 'list-loop' | 'single-loop' | 'random';

export interface PlaylistItem {
  id: string;
  title: string;
  url: string;
  coverUrl?: string;
  author?: string;
  status: ItemStatus;
  position: number;
  lastPositionSeconds: number;
  playCount: number;
  lastPlayedAt: string | null;
}

export interface PlaybackContext {
  mode: PlaybackMode;
  currentItemId: string | null;
  currentPositionSeconds: number;
  randomSeed: number | null;
  randomRound: string[];
}

export interface PlaylistDocument {
  version: 1;
  updatedAt: string;
  activePlaylistId: string | null;
  playlists: LocalPlaylist[];
}

export interface LocalPlaylist {
  id: string;
  name: string;
  sourceUrl: string;
  status: 'active' | 'archived';
  createdAt: string;
  updatedAt: string;
  items: PlaylistItem[];
  playback: PlaybackContext;
}

/** 解析适配器输出的项目 DTO（镜像 Rust ParsedItem）。 */
export interface ParsedItem {
  id: string;
  title: string;
  url: string;
  coverUrl?: string;
  author?: string;
  status: ItemStatus;
  durationSecs?: number;
}

/** 编辑历史事件（镜像 Rust EditEvent；eventId 由 Rust 服务端填充）。 */
export interface EditHistoryEvent {
  eventId?: string;
  timestamp: string;
  eventType: string;
  itemIds: string[];
  playlistId?: string;
  sourcePlaylistUrl?: string;
  snapshot?: PlaylistDocument;
}

/** 播放历史事件（镜像 Rust PlaybackEvent；eventId 由 Rust 服务端填充）。
 *  命名为 *HistoryEvent 以免与 playbackBridge.ts 的 PlaybackEvent 联合类型冲突。 */
export interface PlaybackHistoryEvent {
  eventId?: string;
  timestamp: string;
  eventType: string;
  itemId?: string;
  playlistId?: string;
  sourcePlaylistUrl?: string;
  positionSeconds: number;
  error?: string;
}
