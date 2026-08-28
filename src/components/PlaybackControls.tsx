import { useRef } from 'react';
import type { PlaybackMode } from '../types/playlist';

interface Props {
  playing: boolean;
  mode: PlaybackMode;
  positionSeconds: number;
  durationSeconds: number;
  onToggle: () => void;
  onPrevious: () => void;
  onNext: () => void;
  onMode: (mode: PlaybackMode) => void;
  onSeekRequest: (positionSeconds: number) => void;
  onSeekCommit: (positionSeconds: number) => void;
}

function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) seconds = 0;
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, '0')}`;
}

const MODE_LABELS: Record<PlaybackMode, string> = {
  ordered: '顺序播放',
  'list-loop': '列表循环',
  'single-loop': '单曲循环',
  random: '随机播放',
};

/** 底部播放控制：图标按钮（上一首/播放·暂停/下一首/循环模式）+ 进度条。
 *  进度条拖拽采用 commit-on-release：拖动中只乐观更新 UI（不发 seek IPC），释放/失焦时
 *  才补一次 seek，避免每像素一次 IPC 洪流且不被 progress 回写拽回。
 *  音量延后给 B 站页内播放器，本步不加音量条。 */
export function PlaybackControls({ playing, mode, positionSeconds, durationSeconds, onPrevious, onNext, onToggle, onMode, onSeekRequest, onSeekCommit }: Props) {
  const max = durationSeconds > 0 ? durationSeconds : 0;
  const value = Math.min(positionSeconds, max);
  const pendingSeekRef = useRef<number | null>(null);
  const commit = () => {
    const pos = pendingSeekRef.current;
    if (pos === null) return;
    pendingSeekRef.current = null;
    onSeekCommit(pos);
  };
  return (
    <footer className="controls">
      <button className="ctrl-icon" onClick={onPrevious} title="上一首" aria-label="上一首">
        <svg viewBox="0 0 24 24" width="20" height="20"><path fill="currentColor" d="M6 6h2v12H6zm3.5 6l8.5 6V6z"/></svg>
      </button>
      <button className="ctrl-icon primary" onClick={onToggle} title={playing ? '暂停' : '播放'} aria-label={playing ? '暂停' : '播放'}>
        {playing
          ? <svg viewBox="0 0 24 24" width="22" height="22"><path fill="currentColor" d="M6 5h4v14H6zm8 0h4v14h-4z"/></svg>
          : <svg viewBox="0 0 24 24" width="22" height="22"><path fill="currentColor" d="M8 5v14l11-7z"/></svg>}
      </button>
      <button className="ctrl-icon" onClick={onNext} title="下一首" aria-label="下一首">
        <svg viewBox="0 0 24 24" width="20" height="20"><path fill="currentColor" d="M16 6h2v12h-2zM6 18l8.5-6L6 6z"/></svg>
      </button>
      <input
        type="range"
        className="progress"
        min={0}
        max={max}
        step={1}
        value={value}
        disabled={durationSeconds <= 0}
        onChange={(event) => { pendingSeekRef.current = Number(event.target.value); onSeekRequest(Number(event.target.value)); }}
        onPointerUp={commit}
        onKeyUp={commit}
        onBlur={commit}
      />
      <span className="time-display">{formatTime(positionSeconds)} / {formatTime(durationSeconds)}</span>
      <button className="ctrl-icon mode-btn" onClick={() => {
        const order: PlaybackMode[] = ['ordered', 'list-loop', 'single-loop', 'random'];
        const idx = order.indexOf(mode);
        onMode(order[(idx + 1) % order.length]);
      }} title={MODE_LABELS[mode]} aria-label={MODE_LABELS[mode]}>
        {mode === 'ordered' && <svg viewBox="0 0 24 24" width="20" height="20"><path fill="currentColor" d="M3 5h14v2H3zm0 4h14v2H3zm0 4h10v2H3zm14 2v3l4-3-4-3z"/></svg>}
        {mode === 'list-loop' && <svg viewBox="0 0 24 24" width="20" height="20"><path fill="currentColor" d="M7 7h10v3l4-4-4-4v3H5v6h2zm10 10H7v-3l-4 4 4 4v-3h12v-6h-2z"/></svg>}
        {mode === 'single-loop' && <svg viewBox="0 0 24 24" width="20" height="20"><path fill="currentColor" d="M7 7h10v3l4-4-4-4v3H5v6h2zm10 10H7v-3l-4 4 4 4v-3h12v-6h-2z"/><path stroke="currentColor" stroke-width="1.5" d="M9 11h6"/></svg>}
        {mode === 'random' && <svg viewBox="0 0 24 24" width="20" height="20"><path fill="currentColor" d="M3 7h4l3 3-1.5 1.5L6.5 9H3zm0 10h3.5l8-8H21v2h-5.5l-8 8H3zm14-9v3l4-2.5z"/></svg>}
      </button>
    </footer>
  );
}
