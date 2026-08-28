import type { PlaylistItem } from '../types/playlist';
interface Props { items: PlaylistItem[]; currentItemId: string | null; onPlay: (item: PlaylistItem) => void; onDelete: (id: string) => void }
export function PlaylistQueue({ items, currentItemId, onPlay, onDelete }: Props) {
  if (!items.length) return <div className="empty-state">粘贴 Bilibili 列表地址开始创建本地播放列表</div>;
  // 精简行：只显示名字，双击播放；小 × 删除。无封面大图标。
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
          <button className="row-delete" onClick={() => onDelete(item.id)} aria-label={`删除 ${item.title || item.id}`} title="删除">×</button>
        </article>
      ))}
    </div>
  );
}
