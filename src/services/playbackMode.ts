import type { PlaybackMode, PlaylistItem } from '../types/playlist';

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
