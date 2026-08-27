import type { PlaylistDocument } from '../types/playlist';
import { invoke } from '@tauri-apps/api/core';

export interface StoreAdapter {
  load(): Promise<PlaylistDocument | null>;
  save(document: PlaylistDocument): Promise<void>;
}

export function createMemoryStore(initial: PlaylistDocument | null = null): StoreAdapter {
  let value = initial;
  return {
    async load() { return value; },
    async save(document) { value = document; },
  };
}

export function createTauriStore(): StoreAdapter {
  return {
    load: () => invoke<PlaylistDocument | null>('load_playlists'),
    save: (document) => invoke<void>('save_playlists', { document }),
  };
}
