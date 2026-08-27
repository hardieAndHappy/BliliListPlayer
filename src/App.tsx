import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { PlaybackControls } from './components/PlaybackControls';
import { PlaylistQueue } from './components/PlaylistQueue';
import { validateListUrl } from './services/bilibiliParser';
import { appendEditEvent, appendPlaybackEvent } from './services/historyStore';
import { nextItem, previousItem } from './services/playbackMode';
import { createTauriPlaybackBridge, type PlaybackCommand, type PlaybackEvent } from './services/playbackBridge';
import { reducePlayback, type PlaybackUiState } from './services/playbackReducer';
import { captureAndParse } from './services/parseService';
import { createTauriStore } from './services/playlistStore';
import { BILIBILI_LIST_HINT, focusBilibiliWebview, navigateBilibili, openBilibiliLogin, setBilibiliBounds } from './services/webviewStore';
import type { LocalPlaylist, ParsedItem, PlaylistDocument, PlaylistItem } from './types/playlist';

const emptyPlaylist = (sourceUrl: string, name: string): LocalPlaylist => ({
  id: crypto.randomUUID(), name, sourceUrl, status: 'active', createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), items: [],
  playback: { mode: 'list-loop', currentItemId: null, currentPositionSeconds: 0, randomSeed: null, randomRound: [] },
});

const initialPlaybackState: PlaybackUiState = { playing: false, currentItemId: null, positionSeconds: 0, durationSeconds: 0, error: null };

const buildDocument = (
  playlists: LocalPlaylist[], activePlaylistId: string | null, currentItemId: string | null, mode: PlaylistDocument['playlists'][number]['playback']['mode']
): PlaylistDocument => ({
  version: 1,
  updatedAt: new Date().toISOString(),
  activePlaylistId,
  playlists: playlists.map((playlist) =>
    playlist.id === activePlaylistId
      ? { ...playlist, playback: { ...playlist.playback, currentItemId, mode } }
      : playlist
  ),
});

/** ParsedItem → PlaylistItem：补齐本地字段（位置/续播位/计数/时间）。 */
const toPlaylistItem = (item: ParsedItem, position: number): PlaylistItem => ({
  id: item.id, title: item.title, url: item.url, coverUrl: item.coverUrl, author: item.author,
  status: item.status, position, lastPositionSeconds: 0, playCount: 0, lastPlayedAt: null,
});

export default function App() {
  const [playlists, setPlaylists] = useState<LocalPlaylist[]>([]);
  const [activePlaylistId, setActivePlaylistId] = useState<string | null>(null);
  const [currentItemId, setCurrentItemId] = useState<string | null>(null);
  const [mode, setMode] = useState<PlaylistDocument['playlists'][number]['playback']['mode']>('list-loop');
  const [playback, setPlayback] = useState<PlaybackUiState>(initialPlaybackState);
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; playlistId: string } | null>(null);
  // 当前正在播放的视频所属列表 id（与选中列表 activePlaylistId 区分）。
  // 侧边栏据此给"正在播放"的列表加播放标记；切列表浏览时若正在播放则不隐藏 webview。
  const [playingPlaylistId, setPlayingPlaylistId] = useState<string | null>(null);
  // 队列区宽度（px），由分割栏拖拽控制；播放区占剩余宽度。持久到 localStorage 跨会话保留。
  const [queueWidth, setQueueWidth] = useState<number>(() => {
    const saved = Number(localStorage.getItem('queueWidth'));
    return saved > 0 ? saved : 560;
  });
  const storageReady = useRef(false);
  const bridgeRef = useRef(createTauriPlaybackBridge());
  const playerRef = useRef<HTMLElement>(null);
  // 子 webview 只在「播放/登录」时显示；导入/预览/编辑列表时隐藏，避免原生层 z-order 挡住 HTML UI。
  const webviewVisibleRef = useRef(false);

  /** 测量播放区 DOM 矩形并上报 Rust，校准嵌入子 webview 位置/尺寸。
   *  webviewVisibleRef=false（导入/预览/编辑）或无播放区时上报 0，Rust 隐藏子 webview，
   *  使导入预览等 HTML 浮层不被原生 webview 遮挡。true 时上报真实矩形 → Rust 显示。
   *  body 不滚动（app-shell 恰为 100vh），viewport 相对坐标 == OS 窗口客户区坐标。 */
  const measureAndReportBounds = useCallback((): Promise<void> => {
    const el = playerRef.current;
    if (!el || !webviewVisibleRef.current) {
      return setBilibiliBounds(0, 0, 0, 0).catch(() => {});
    }
    const rect = el.getBoundingClientRect();
    return setBilibiliBounds(rect.left, rect.top, rect.width, rect.height).catch((error) => {
      setNotice(`校准播放区失败：${String(error)}`);
    });
  }, []);

  // 原生子 WebView 位于 HTML 之上，打开浏览器原生 prompt/confirm 前必须先隐藏，
  // 否则对话框落在播放器区域时会被 WebView 盖住。
  const hideBilibiliWebview = useCallback(async (): Promise<void> => {
    webviewVisibleRef.current = false;
    await setBilibiliBounds(0, 0, 0, 0).catch(() => {});
  }, []);

  const active = playlists.find((playlist) => playlist.id === activePlaylistId) ?? null;
  const items = active?.items ?? [];
  const current = useMemo(() => items.find((item) => item.id === currentItemId), [items, currentItemId]);

  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    createTauriStore().load().then((document) => {
      if (document) { setPlaylists(document.playlists); setActivePlaylistId(document.activePlaylistId); }
      storageReady.current = true;
    }).catch((error) => setNotice(`读取本地列表失败：${String(error)}`));
  }, []);

  useEffect(() => {
    if (!storageReady.current) return;
    createTauriStore().save(buildDocument(playlists, activePlaylistId, currentItemId, mode))
      .catch((error) => setNotice(`保存本地列表失败：${String(error)}`));
  }, [activePlaylistId, playlists, currentItemId, mode]);

  const sendCommand = (command: PlaybackCommand) =>
    bridgeRef.current.send(command).catch((error) => setNotice(`播放控制失败：${String(error)}`));

  const goNext = (fromId: string | null = currentItemId) => {
    const result = nextItem(items, fromId, mode);
    if (!result.itemId) return;
    const next = items.find((item) => item.id === result.itemId);
    if (!next) return;
    setCurrentItemId(result.itemId);
    sendCommand({ type: 'load', url: next.url, positionSeconds: next.lastPositionSeconds });
  };

  const goPrevious = () => {
    const result = previousItem(items, currentItemId, mode);
    if (!result.itemId) return;
    const prev = items.find((item) => item.id === result.itemId);
    if (!prev) return;
    setCurrentItemId(result.itemId);
    sendCommand({ type: 'load', url: prev.url, positionSeconds: prev.lastPositionSeconds });
  };

  const recordPlayback = (event: PlaybackEvent) => {
    appendPlaybackEvent({
      timestamp: new Date().toISOString(),
      eventType: event.type,
      itemId: event.itemId,
      playlistId: active?.id,
      sourcePlaylistUrl: active?.sourceUrl,
      positionSeconds: 'positionSeconds' in event ? event.positionSeconds : 0,
      error: event.type === 'error' ? event.message : undefined,
    }).catch((error) => setNotice(`记录播放历史失败：${String(error)}`));
  };

  const savePosition = (itemId: string, positionSeconds: number) => {
    setPlaylists((value) => value.map((playlist) =>
      playlist.id === activePlaylistId
        ? { ...playlist, items: playlist.items.map((item) => item.id === itemId ? { ...item, lastPositionSeconds: positionSeconds } : item), updatedAt: new Date().toISOString() }
        : playlist
    ));
  };

  // 桥接事件 → 纯状态机 + 副作用（§5.3）。playing 由 started 驱动单一真相，不在此同步置位。
  const handleEvent = (event: PlaybackEvent) => {
    setPlayback((state) => reducePlayback(state, event));
    switch (event.type) {
      case 'started':
        recordPlayback(event);
        break;
      case 'paused':
        recordPlayback(event);
        savePosition(event.itemId, event.positionSeconds);
        break;
      case 'ended':
        recordPlayback(event);
        savePosition(event.itemId, 0);
        goNext(event.itemId);
        break;
      case 'error':
        recordPlayback(event);
        setNotice(`播放失败：${event.message}`);
        break;
      case 'progress':
        break;
    }
  };
  const handleEventRef = useRef(handleEvent);
  useEffect(() => { handleEventRef.current = handleEvent; });

  // 右键菜单：任意点击或 Esc 关闭。
  useEffect(() => {
    if (!ctxMenu) return;
    const close = () => setCtxMenu(null);
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setCtxMenu(null); };
    window.addEventListener('click', close);
    window.addEventListener('keydown', onKey);
    return () => { window.removeEventListener('click', close); window.removeEventListener('keydown', onKey); };
  }, [ctxMenu]);

  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    const unsubscribe = bridgeRef.current.subscribe((event) => handleEventRef.current(event));
    return unsubscribe;
  }, []);

  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    let unsubscribe: UnlistenFn | undefined;
    listen<{ url: string; loginState: string }>('bilibili://page-state', (event) => {
      if (event.payload.loginState === 'guest') setNotice('检测到未登录或被风控，请在播放区登录 B 站后再导入');
    }).then((un) => { unsubscribe = un; });
    return () => { unsubscribe?.(); };
  }, []);

  // 播放区矩形上报：挂载即测 + 元素尺寸变化（ResizeObserver）+ 窗口缩放触发重测。
  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    measureAndReportBounds();
    const el = playerRef.current;
    const observer = el ? new ResizeObserver(() => measureAndReportBounds()) : null;
    if (el && observer) observer.observe(el);
    const onResize = () => measureAndReportBounds();
    window.addEventListener('resize', onResize);
    return () => { observer?.disconnect(); window.removeEventListener('resize', onResize); };
  }, [measureAndReportBounds]);

  // 窗口缩放后 Rust 发此事件；延一帧待 CSS 重排完成再测，避免测到过渡中尺寸。
  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    let unsubscribe: UnlistenFn | undefined;
    listen('bilibili://window-resized', () => {
      requestAnimationFrame(() => measureAndReportBounds());
    }).then((un) => { unsubscribe = un; });
    return () => { unsubscribe?.(); };
  }, [measureAndReportBounds]);

  const importList = async (input: string) => {
    let url: URL;
    try {
      url = validateListUrl(input);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : '地址无效');
      return;
    }
    const normalized = url.toString();
    const sameSource = playlists.some((playlist) => playlist.sourceUrl === normalized);
    const existing = sameSource ? playlists.find((playlist) => playlist.sourceUrl === normalized) ?? null : null;
    // 前置快检：同来源列表已存在时，先确认是否重新抓取检查更新，避免每次都抓整页（耗时/风控风险）。
    if (existing) {
      await hideBilibiliWebview();
      if (!window.confirm(`列表「${existing.name}」已从该来源导入过（${existing.items.length} 项）。是否重新抓取以检查更新？`)) {
        return;
      }
    }
    setBusy(true);
    setNotice('正在打开 Bilibili 列表页并解析…');
    try {
      // 导入/预览期间隐藏子 webview，避免原生层遮挡预览浮层；页面在隐藏 webview 后台加载+抓取。
      await hideBilibiliWebview();
      await navigateBilibili(normalized);
      const { items: parsedItems, listTitle } = await captureAndParse(normalized);
      // 不弹预览，直接落库：同来源更新（自动去重追加），否则新建列表。
      const added = applyImport(parsedItems, normalized, existing, listTitle);
      setNotice(`已导入 ${added} 项${existing ? '（已自动跳过已存在）' : ''}`);
    } catch (error) {
      setNotice(`导入失败：${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  /// 新增列表：弹窗输入 Bilibili 列表地址，确认后导入。
  const handleAddList = async () => {
    await hideBilibiliWebview();
    const input = window.prompt(`粘贴 Bilibili 列表或视频地址（${BILIBILI_LIST_HINT}）`);
    if (!input?.trim()) return;
    void importList(input.trim());
  };

  const applyImport = (parsedItems: ParsedItem[], sourceUrl: string, existing: LocalPlaylist | null, listTitle: string | null): number => {
    const newItems = parsedItems.map((item, index) => toPlaylistItem(item, index));
    let targetId: string | null = null;
    let added = 0;
    if (existing) {
      setPlaylists((value) => value.map((playlist) => {
        if (playlist.id !== existing.id) return playlist;
        const existingIds = new Set(playlist.items.map((item) => item.id.toLowerCase()));
        const additions = newItems
          .filter((item) => !existingIds.has(item.id.toLowerCase()))
          .map((item, index) => ({ ...item, position: playlist.items.length + index }));
        added = additions.length;
        return { ...playlist, items: [...playlist.items, ...additions], updatedAt: new Date().toISOString() };
      }));
      setActivePlaylistId(existing.id);
      targetId = existing.id;
    } else {
      const fallbackName = `列表 ${sourceUrl.match(/\/list\/(\d+)/)?.[1] ?? playlists.length + 1}`;
      const playlist = emptyPlaylist(sourceUrl, listTitle || fallbackName);
      playlist.items = newItems;
      added = newItems.length;
      setPlaylists((value) => [...value, playlist]);
      setActivePlaylistId(playlist.id);
      targetId = playlist.id;
    }
    appendEditEvent({
      timestamp: new Date().toISOString(),
      eventType: 'import',
      itemIds: newItems.map((item) => item.id),
      playlistId: targetId ?? undefined,
      sourcePlaylistUrl: sourceUrl,
    }).catch((error) => setNotice(`记录编辑历史失败：${String(error)}`));
    return added;
  };

  const updateItems = (update: (value: PlaylistItem[]) => PlaylistItem[]) =>
    setPlaylists((value) => value.map((playlist) =>
      playlist.id === activePlaylistId
        ? { ...playlist, items: update(playlist.items), updatedAt: new Date().toISOString() }
        : playlist
    ));

  const handleDelete = (id: string) => {
    const snapshot = buildDocument(playlists, activePlaylistId, currentItemId, mode);
    updateItems((value) => value.filter((item) => item.id !== id));
    appendEditEvent({
      timestamp: new Date().toISOString(),
      eventType: 'delete',
      itemIds: [id],
      playlistId: active?.id,
      sourcePlaylistUrl: active?.sourceUrl,
      snapshot,
    }).catch((error) => setNotice(`记录编辑历史失败：${String(error)}`));
  };

  /// 删除整个播放列表（来源）：右键侧边栏列表项触发，删除前二次确认（规格 L81），
  /// 并写一条 delete 编辑历史（itemIds=该列表全部项，snapshot=删除前文档）。
  const handleDeletePlaylist = async (playlistId: string) => {
    const target = playlists.find((playlist) => playlist.id === playlistId);
    if (!target) return;
    await hideBilibiliWebview();
    if (!window.confirm(`确定删除列表「${target.name}」？此操作不可撤销。`)) return;
    const snapshot = buildDocument(playlists, activePlaylistId, currentItemId, mode);
    setPlaylists((value) => value.filter((playlist) => playlist.id !== playlistId));
    if (activePlaylistId === playlistId) {
      setActivePlaylistId(null);
      setCurrentItemId(null);
      setPlayback(initialPlaybackState);
    }
    if (playingPlaylistId === playlistId) {
      setPlayingPlaylistId(null);
      void hideBilibiliWebview();
    }
    appendEditEvent({
      timestamp: new Date().toISOString(),
      eventType: 'delete',
      itemIds: target.items.map((item) => item.id),
      playlistId,
      sourcePlaylistUrl: target.sourceUrl,
      snapshot,
    }).catch((error) => setNotice(`记录编辑历史失败：${String(error)}`));
    setNotice(`已删除列表「${target.name}」`);
  };

  /// 重命名播放列表：右键侧边栏列表项触发，prompt 输入新名，写一条 rename 编辑历史。
  const handleRenamePlaylist = async (playlistId: string) => {
    const target = playlists.find((playlist) => playlist.id === playlistId);
    if (!target) return;
    await hideBilibiliWebview();
    const next = window.prompt('重命名列表', target.name)?.trim();
    if (!next || next === target.name) return;
    setPlaylists((value) => value.map((playlist) =>
      playlist.id === playlistId ? { ...playlist, name: next, updatedAt: new Date().toISOString() } : playlist
    ));
    appendEditEvent({
      timestamp: new Date().toISOString(),
      eventType: 'rename',
      itemIds: [],
      playlistId,
      sourcePlaylistUrl: target.sourceUrl,
    }).catch((error) => setNotice(`记录编辑历史失败：${String(error)}`));
  };

  /** 分割栏拖拽：按鼠标移动增减 queueWidth，钳制到 [240, 窗口宽-568]（留 sidebar 200 + splitter 8 + 播放区最小 360），并实时重测 webview bounds。
   *  mousemove/mouseup 挂 window 以便鼠标移出分割栏仍能跟踪。 */
  const onSplitterMouseDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = queueWidth;
    const max = window.innerWidth - 200 - 8 - 360;
    let latest = startWidth;
    const onMove = (ev: MouseEvent) => {
      latest = Math.min(Math.max(startWidth + (ev.clientX - startX), 240), max);
      setQueueWidth(latest);
    };
    const onUp = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
      localStorage.setItem('queueWidth', String(latest));
      void measureAndReportBounds();
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  const handlePlay = (item: PlaylistItem) => {
    setCurrentItemId(item.id);
    setPlayingPlaylistId(activePlaylistId);
    webviewVisibleRef.current = true;
    void measureAndReportBounds();
    sendCommand({ type: 'load', url: item.url, positionSeconds: item.lastPositionSeconds });
  };

  const onToggle = () => sendCommand(playback.playing ? { type: 'pause' } : { type: 'play' });
  const onSeek = (positionSeconds: number) => sendCommand({ type: 'seek', positionSeconds });

  // 打开登录页 / 显示嵌入区前先上报矩形，确保子 webview 创建即落在播放区。
  const handleOpenLogin = async () => { webviewVisibleRef.current = true; await measureAndReportBounds(); openBilibiliLogin(); };
  const handleShowWebview = async () => { webviewVisibleRef.current = true; await measureAndReportBounds(); focusBilibiliWebview(); };

  return (
    <div className="app-shell" style={{ '--queue-w': `${queueWidth}px` } as React.CSSProperties}>
      <aside className="sidebar">
        <div className="sidebar-head">
          <strong>已保存列表</strong>
        </div>
        <div className="sidebar-scroll">
          <button className="add-row" onClick={handleAddList} disabled={busy} title="新增列表">＋ 新增列表</button>
          {playlists.map((playlist) => {
            const isActive = playlist.id === activePlaylistId;
            const isPlaying = playlist.id === playingPlaylistId;
            return (
              <button
                key={playlist.id}
                className={isActive ? 'active' : ''}
                onClick={() => {
                  setActivePlaylistId(playlist.id); setCurrentItemId(playlist.playback.currentItemId); setMode(playlist.playback.mode); setPlayback(initialPlaybackState);
                  // 列表区切换只改变本地队列选择，不导航、不刷新、不隐藏当前 WebView。
                }}
                onContextMenu={(e) => { e.preventDefault(); setCtxMenu({ x: e.clientX, y: e.clientY, playlistId: playlist.id }); }}
              >
                {isPlaying && <span className="playing-mark" aria-hidden="true">▶</span>}
                {playlist.name}（{playlist.items.length}）
              </button>
            );
          })}
          <button>最近播放</button>
          <button>编辑历史</button>
        </div>
      </aside>
      <section className="queue">
        <h1>{active?.name || '我的播放列表'}</h1>
        {active && <small className="source-url">来源：{active.sourceUrl}</small>}
        {notice && <div className="notice">{notice}</div>}
        <PlaylistQueue items={items} currentItemId={currentItemId} onPlay={handlePlay} onDelete={handleDelete} />
      </section>
      <div className="splitter" onMouseDown={onSplitterMouseDown} title="拖拽调整宽度" />
      <section className="player" ref={playerRef}>
        <div className="webview-placeholder">
          <strong>{current?.title || 'Bilibili WebView 播放区'}</strong>
          <span>正常网页登录与在线播放，不下载媒体。</span>
          <div className="player-actions">
            <button onClick={handleOpenLogin}>打开登录页</button>
            <button onClick={handleShowWebview}>显示 WebView</button>
          </div>
        </div>
      </section>
      <PlaybackControls
        playing={playback.playing}
        mode={mode}
        positionSeconds={playback.positionSeconds}
        durationSeconds={playback.durationSeconds}
        onToggle={onToggle}
        onPrevious={goPrevious}
        onNext={() => goNext()}
        onMode={setMode}
        onSeek={onSeek}
      />
      {ctxMenu && (
        <div className="ctx-menu" style={{ left: ctxMenu.x, top: ctxMenu.y }}>
          <button onClick={() => { void handleRenamePlaylist(ctxMenu.playlistId); setCtxMenu(null); }}>重命名</button>
          <button className="danger" onClick={() => { void handleDeletePlaylist(ctxMenu.playlistId); setCtxMenu(null); }}>删除列表</button>
        </div>
      )}
    </div>
  );
}
