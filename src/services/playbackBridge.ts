import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type PlaybackCommand =
  | { type: 'load'; url: string; positionSeconds: number }
  | { type: 'play' | 'pause' | 'next' | 'previous' }
  | { type: 'seek'; positionSeconds: number };

export type PlaybackEvent =
  | { type: 'started' | 'ended' | 'paused'; itemId: string; positionSeconds: number }
  | { type: 'error'; itemId: string; message: string }
  | { type: 'progress'; itemId: string; positionSeconds: number; durationSeconds: number };

export interface PlaybackBridge {
  send(command: PlaybackCommand): Promise<void>;
  subscribe(listener: (event: PlaybackEvent) => void): () => void;
}

export function isAllowedBilibiliUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return url.protocol === 'https:' && (url.hostname === 'www.bilibili.com' || url.hostname.endsWith('.bilibili.com'));
  } catch {
    return false;
  }
}

export function createTauriPlaybackBridge(): PlaybackBridge {
  return {
    async send(command) { await invoke('send_playback_command', { command }); },
    subscribe(listener) {
      let un: UnlistenFn | undefined;
      listen<PlaybackEvent>('bilibili://playback-event', (e) => listener(e.payload)).then((u) => (un = u));
      return () => { un?.(); };
    },
  };
}
