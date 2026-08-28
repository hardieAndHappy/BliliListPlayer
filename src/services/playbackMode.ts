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
    const remaining = playable.map((item) => item.id).filter((id) => id !== currentId && !round.includes(id));
    if (!remaining.length) return nextItem(playable, null, 'random', [], randomSeed + 1);
    const index = Math.abs(randomSeed + round.length) % remaining.length;
    const itemId = remaining[index];
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
    const remaining = playable.map((item) => item.id).filter((id) => id !== currentId && !round.includes(id));
    if (!remaining.length) return previousItem(playable, null, 'random', [], randomSeed + 1);
    const index = Math.abs(randomSeed + round.length) % remaining.length;
    const itemId = remaining[index];
    return { itemId, round: [...round, itemId] };
  }
  const prevIndex = currentIndex < 0 ? playable.length - 1 : currentIndex - 1;
  if (prevIndex < 0) {
    return mode === 'list-loop' ? { itemId: playable[playable.length - 1].id, round } : { itemId: null, round };
  }
  return { itemId: playable[prevIndex].id, round };
}
