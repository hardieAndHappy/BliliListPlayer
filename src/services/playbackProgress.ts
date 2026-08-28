import type { LocalPlaylist } from '../types/playlist';

export function getPlaybackStartPosition(): number {
  return 0;
}

export function updatePlaylistItemPosition(
  playlists: LocalPlaylist[],
  playlistId: string | null,
  itemId: string,
  positionSeconds: number,
): LocalPlaylist[] {
  if (!playlistId) return playlists;
  return playlists.map((playlist) =>
    playlist.id === playlistId
      ? {
          ...playlist,
          items: playlist.items.map((item) =>
            item.id === itemId ? { ...item, lastPositionSeconds: positionSeconds } : item
          ),
          updatedAt: new Date().toISOString(),
        }
      : playlist
  );
}
