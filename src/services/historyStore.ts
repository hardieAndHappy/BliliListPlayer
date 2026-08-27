import { invoke } from '@tauri-apps/api/core';
import type { EditHistoryEvent, PlaybackHistoryEvent } from '../types/playlist';

/** 追加编辑历史事件（§5.4）。不传 eventId，由 Rust 命令服务端填充。 */
export function appendEditEvent(event: EditHistoryEvent): Promise<void> {
  return invoke<void>('append_edit_event', { event });
}

/** 追加播放历史事件（§5.4）。不传 eventId，由 Rust 命令服务端填充。 */
export function appendPlaybackEvent(event: PlaybackHistoryEvent): Promise<void> {
  return invoke<void>('append_playback_event', { event });
}
