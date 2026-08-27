export interface PlaybackUiState {
  playing: boolean;
  currentItemId: string | null;
  positionSeconds: number;
  durationSeconds: number;
  error: string | null;
}

export type PlaybackAction =
  | { type: 'started'; itemId: string; positionSeconds: number }
  | { type: 'progress'; itemId: string; positionSeconds: number; durationSeconds: number }
  | { type: 'paused'; itemId: string; positionSeconds: number }
  | { type: 'ended'; itemId: string; positionSeconds: number }
  | { type: 'error'; itemId: string; message: string };

export function reducePlayback(state: PlaybackUiState, action: PlaybackAction): PlaybackUiState {
  switch (action.type) {
    case 'started':
      return { ...state, playing: true, currentItemId: action.itemId, positionSeconds: action.positionSeconds, error: null };
    case 'progress':
      return { ...state, positionSeconds: action.positionSeconds, durationSeconds: action.durationSeconds };
    case 'paused':
      return { ...state, playing: false, positionSeconds: action.positionSeconds };
    case 'ended':
      return { ...state, playing: false, positionSeconds: action.positionSeconds };
    case 'error':
      return { ...state, playing: false, error: action.message };
    default:
      return state;
  }
}
