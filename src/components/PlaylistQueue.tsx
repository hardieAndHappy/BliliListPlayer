import type { PlaylistItem } from '../types/playlist';
interface Props { items: PlaylistItem[]; currentItemId: string | null; refreshingIds: Set<string>; onPlay: (item: PlaylistItem) => void; onRefresh: (item: PlaylistItem) => void; onDelete: (id: string) => void }
export function PlaylistQueue({ items, currentItemId, refreshingIds, onPlay, onRefresh, onDelete }: Props) {
  if (!items.length) return <div className="empty-state">粘贴 Bilibili 列表地址开始创建本地播放列表</div>;
  // 精简行：序号 + 播放标记 + 名字 + 刷新名字 + ×删除。双击行播放。
  return (
    <div className="playlist-list">
      {items.map((item, index) => (
        <article
          className={`playlist-row ${currentItemId === item.id ? 'active' : ''}`}
          data-current={currentItemId === item.id ? 'true' : undefined}
          key={item.id}
          onDoubleClick={() => onPlay(item)}
          title="双击播放"
        >
          <span className="row-index">{index + 1}</span>
          <span className="row-playing-slot">
            {currentItemId === item.id && <span className="row-playing-icon" aria-label="正在播放" title="正在播放">▶</span>}
          </span>
          <strong className="row-title">{item.title || item.id}</strong>
          <button
            className="row-refresh"
            onClick={(e) => { e.stopPropagation(); onRefresh(item); }}
            disabled={refreshingIds.has(item.id)}
            aria-label={`刷新 ${item.title || item.id} 的名字`}
            title="刷新名字"
          >⟳</button>
          <button className="row-delete" onClick={(e) => { e.stopPropagation(); onDelete(item.id); }} aria-label={`删除 ${item.title || item.id}`} title="删除">×</button>
        </article>
      ))}
    </div>
  );
}
