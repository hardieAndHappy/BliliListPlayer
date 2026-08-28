import type { LocalPlaylist, PlaybackMode, PlaylistItem } from '../types/playlist';

export interface PlaybackNavigationContext {
  playlistId: string;
  items: PlaylistItem[];
  currentItemId: string | null;
  mode: PlaybackMode;
}

export function resolvePlaybackNavigationContext(
  playlists: LocalPlaylist[],
  activePlaylistId: string | null,
  playingPlaylistId: string | null,
  activeCurrentItemId: string | null,
  activeMode: PlaybackMode,
  playingItemId: string | null = null,
  playingMode: PlaybackMode | null = null,
): PlaybackNavigationContext | null {
  const isPlayingContext = playingPlaylistId !== null;
  const sourceId = playingPlaylistId ?? activePlaylistId;
  const source = playlists.find((playlist) => playlist.id === sourceId);
  if (!source) return null;
  return {
    playlistId: source.id,
    items: source.items,
    currentItemId: isPlayingContext ? (playingItemId ?? source.playback.currentItemId) : activeCurrentItemId,
    mode: isPlayingContext ? (playingMode ?? source.playback.mode) : activeMode,
  };
}

export function nextItem(items: PlaylistItem[], currentId: string | null, mode: PlaybackMode, round: string[] = [], randomSeed = 1) {
  const playable = items.filter((item) => item.status !== 'invalid' && item.status !== 'deleted');
  if (!playable.length) return { itemId: null, round: [] };
  const currentIndex = playable.findIndex((item) => item.id === currentId);
  if (mode === 'single-loop' && currentId && currentIndex >= 0) return { itemId: currentId, round };
  if (mode === 'random') {
    // 真随机：Math.random() 等概率抽一首，允许连播同一首。round/randomSeed 仅为兼容旧调用签名
    // 保留（落盘的死字段），不再参与选曲——旧实现用确定性偏移 (randomSeed+round.length)%N，
    // 既不随机、round 不重复逻辑也因调用方从不传参而从未生效。
    const pool = playable.map((item) => item.id).filter((id) => id !== currentId);
    if (!pool.length) return { itemId: currentId, round };
    const itemId = pool[Math.floor(Math.random() * pool.length)];
    return { itemId, round: [...round, itemId] };
  }
  const nextIndex = currentIndex < 0 ? 0 : currentIndex + 1;
  if (nextIndex >= playable.length) {
    return mode === 'list-loop' ? { itemId: playable[0].id, round } : { itemId: null, round };
  }
  return { itemId: playable[nextIndex].id, round };
}

export function previousItem(items: PlaylistItem[], currentId: string | null, mode: PlaybackMode, round: string[] = [], randomSeed = 1) {
  const playable = items.filter((item) => item.status !== 'invalid' && item.status !== 'deleted');
  if (!playable.length) return { itemId: null, round: [] };
  const currentIndex = playable.findIndex((item) => item.id === currentId);
  if (mode === 'single-loop' && currentId && currentIndex >= 0) return { itemId: currentId, round };
  if (mode === 'random') {
    // 真随机（与 nextItem 同）：等概率抽一首，允许连播同一首。
    const pool = playable.map((item) => item.id).filter((id) => id !== currentId);
    if (!pool.length) return { itemId: currentId, round };
    const itemId = pool[Math.floor(Math.random() * pool.length)];
    return { itemId, round: [...round, itemId] };
  }
  const prevIndex = currentIndex < 0 ? playable.length - 1 : currentIndex - 1;
  if (prevIndex < 0) {
    return mode === 'list-loop' ? { itemId: playable[playable.length - 1].id, round } : { itemId: null, round };
  }
  return { itemId: playable[prevIndex].id, round };
}
