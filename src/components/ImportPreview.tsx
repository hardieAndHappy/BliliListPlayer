import { useMemo, useState } from 'react';
import type { ParsedItem } from '../types/playlist';
import { selectAll, toggleSelect } from '../services/importSelection';

interface ImportPreviewProps {
  items: ParsedItem[];
  existingIds: string[];
  sameSource: boolean;
  onConfirm: (selected: ParsedItem[], mode: 'update' | 'new') => void;
  onCancel: () => void;
}

const STATUS_LABEL: Record<string, string> = {
  playable: '可播放',
  pending: '待定',
  invalid: '无效',
};

/** 导入预览弹层（§5.1）：卡片列表 + 全选/取消 + sameSource 时「更新现有/另存为新」。
 *  重复项（id 命中 existingIds，小写比较）禁用勾选并标记「已存在」；无效项保留可勾选。 */
export function ImportPreview({ items, existingIds, sameSource, onConfirm, onCancel }: ImportPreviewProps) {
  const existingSet = useMemo(() => new Set(existingIds.map((id) => id.toLowerCase())), [existingIds]);
  // 默认全选所有非重复项（含无效项，§5.1 L84）。
  const [selected, setSelected] = useState<Set<string>>(() => selectAll(items, existingIds));
  const [mode, setMode] = useState<'update' | 'new'>(sameSource ? 'update' : 'new');

  const selectable = useMemo(() => items.filter((item) => !existingSet.has(item.id.toLowerCase())), [items, existingSet]);
  const allSelected = selectable.length > 0 && selectable.every((item) => selected.has(item.id));

  const toggle = (item: ParsedItem) => {
    if (existingSet.has(item.id.toLowerCase())) return;
    setSelected((prev) => toggleSelect(prev, item.id));
  };
  const toggleAll = () => {
    setSelected((prev) => (selectable.length > 0 && selectable.every((item) => prev.has(item.id)) ? new Set<string>() : selectAll(items, existingIds)));
  };
  const confirm = () => {
    const chosen = items.filter((item) => selected.has(item.id));
    if (chosen.length) onConfirm(chosen, mode);
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-card" onClick={(event) => event.stopPropagation()}>
        <header className="modal-head">
          <strong>预览导入（共 {items.length} 项）</strong>
          <button onClick={onCancel}>✕</button>
        </header>
        <div className="modal-toolbar">
          <button onClick={toggleAll}>{allSelected ? '取消全选' : '全选'}</button>
          {sameSource && (
            <span className="modal-mode">
              <label><input type="radio" checked={mode === 'update'} onChange={() => setMode('update')} /> 更新现有</label>
              <label><input type="radio" checked={mode === 'new'} onChange={() => setMode('new')} /> 另存为新</label>
            </span>
          )}
        </div>
        <ul className="import-list">
          {items.map((item) => {
            const dup = existingSet.has(item.id.toLowerCase());
            return (
              <li key={item.id} className={`import-row${dup ? ' disabled' : ''}`}>
                <input type="checkbox" disabled={dup} checked={selected.has(item.id)} onChange={() => toggle(item)} />
                <span className="import-title" title={item.url}>{item.title || item.id}</span>
                <span className={`badge badge-${item.status}`}>{STATUS_LABEL[item.status] ?? item.status}</span>
                {dup && <span className="dup-tag">已存在</span>}
              </li>
            );
          })}
        </ul>
        <footer className="modal-foot">
          <span className="import-count">已选 {selected.size} 项</span>
          <button onClick={onCancel}>取消</button>
          <button className="primary" disabled={selected.size === 0} onClick={confirm}>导入</button>
        </footer>
      </div>
    </div>
  );
}
