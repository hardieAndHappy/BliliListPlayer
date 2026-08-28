//! Bilibili WebView 播放桥接（第 ③ 步）。
//!
//! 在受控 WebView 内加载 Bilibili 页面，注入脚本捕获播放事件与列表 HTML，
//! 经 Tauri 事件桥转发给前端。所有命令为 `async fn` 以避 Windows 主线程
//! `WebviewWindowBuilder::build()` 死锁（wry#583）。

use crate::model::PlaybackCommandDto;
use crate::parser::{self, ParseError};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{
    webview::PageLoadEvent, AppHandle, Emitter, Listener, LogicalPosition, LogicalSize, Manager,
    State, Webview, WebviewBuilder, WebviewUrl,
};
use url::Url;

pub const BILI_LABEL: &str = "bilibili";
const BILI_HOME: &str = "https://www.bilibili.com";
const BILI_MEDIALIST_API: &str = "https://api.bilibili.com/x/v2/medialist/resource/list";
const MAX_CAPTURE_HTML: usize = 20 * 1024 * 1024; // 20MB

fn is_allowed_bili_host(host: &str) -> bool {
    matches!(
        host,
        "www.bilibili.com" | "bilibili.com" | "passport.bilibili.com"
    )
}

fn page_access_state(url: &str) -> &'static str {
    match Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
    {
        Some(host) if host == "passport.bilibili.com" => "verification-required",
        _ => "ready",
    }
}

/// 比较两个 URL 是否指向同一列表页（host+path），忽略 query/fragment 与末尾斜杠。
/// B 站常规范化 query（如去掉 oid/bvid），且会把视频页 `/video/BV1` 规范化为 `/video/BV1/`，
/// 精确比较会让 capture 与播放意图永不触发（pending_seek/pending_play 一直匹配不上）。
fn same_list_page(a: &str, b: &str) -> bool {
    match (Url::parse(a).ok(), Url::parse(b).ok()) {
        (Some(x), Some(y)) => x.host_str() == y.host_str() && normalize_path(x.path()) == normalize_path(y.path()),
        _ => a == b,
    }
}

/// 去掉路径末尾 `/`（保留根 `/`），使 `/video/BV1` 与 `/video/BV1/` 视作同一页。
fn normalize_path(path: &str) -> &str {
    if path.len() > 1 && path.ends_with('/') {
        &path[..path.len() - 1]
    } else {
        path
    }
}

const BRIDGE_INIT: &str = r#"(function(){
  if (window.__BILI_BRIDGE__) return; window.__BILI_BRIDGE__ = true;
  function allowedHost(h){ return h==='www.bilibili.com'||h==='bilibili.com'||h==='passport.bilibili.com'||h.endsWith('.bilibili.com'); }
  function emit(event, payload){
    try { window.__TAURI_INTERNALS__.invoke('plugin:event|emit', { event: event, payload: payload }); } catch(e){}
  }
  if (window.location.protocol !== 'https:' || !allowedHost(window.location.hostname)) return;
  if (!window.__BILI_LIST_PLAYER_AUTOPLAY_GUARD__) {
    window.__BILI_LIST_PLAYER_AUTOPLAY_GUARD__ = true;
    window.__BILI_LIST_PLAYER_PLAY_ALLOWANCE__ = 0;
    var nativePlay = HTMLMediaElement.prototype.play;
    HTMLMediaElement.prototype.play = function(){
      if (window.__BILI_LIST_PLAYER_PLAY_ALLOWANCE__ > 0) {
        window.__BILI_LIST_PLAYER_PLAY_ALLOWANCE__--;
        return nativePlay.apply(this, arguments);
      }
      try { this.pause(); } catch(e) {}
      return Promise.reject(new DOMException('Autoplay disabled by BiliListPlayer', 'NotAllowedError'));
    };
    function disableAutoplay(node) {
      if (node && node instanceof HTMLMediaElement) node.autoplay = false;
    }
    document.querySelectorAll('video,audio').forEach(disableAutoplay);
    new MutationObserver(function(records){
      records.forEach(function(record){
        for (var i=0;i<record.addedNodes.length;i++) {
          var node = record.addedNodes[i];
          disableAutoplay(node);
          if (node && node.querySelectorAll) node.querySelectorAll('video,audio').forEach(disableAutoplay);
        }
      });
    }).observe(document.documentElement, { childList: true, subtree: true });
  }
  emit('bili://page-loaded', { url: window.location.href });
  var SELECTORS = 'bwp-video, video, .bpx-player-container video, .bpx-player-video-wrap video, .bilibili-player video';
  var deadline = Date.now() + 60000;
  function hookVideo(v){
    if (v.__BILI_HOOKED__) return; v.__BILI_HOOKED__ = true;
    ['play','pause','ended','error','timeupdate'].forEach(function(t){
      v.addEventListener(t, function(){
        var payload = { type: t, itemId: window.location.pathname, positionSeconds: v.currentTime || 0 };
        if (isFinite(v.duration) && v.duration > 0) payload.durationSeconds = v.duration;
        emit('bili://video-event', payload);
      });
    });
    // DASH 流尾部未缓冲时原生 ended 可能不触发，用 timeupdate 兜底：接近末尾即补发 ended，
    // 与原生 ended 去重（__BILI_ENDING__）。重新播放时复位标志。
    v.addEventListener('timeupdate', function(){
      if (isFinite(v.duration) && v.duration > 0 && (v.duration - (v.currentTime || 0)) < 0.5 && !v.__BILI_ENDING__) {
        v.__BILI_ENDING__ = true;
        emit('bili://video-event', { type: 'ended', itemId: window.location.pathname, positionSeconds: v.currentTime || 0, durationSeconds: v.duration });
      }
    });
    v.addEventListener('play', function(){ v.__BILI_ENDING__ = false; });
  }
  function tryHook(){
    var nodes = document.querySelectorAll(SELECTORS);
    for (var i=0;i<nodes.length;i++) hookVideo(nodes[i]);
    if (Date.now() < deadline) setTimeout(tryHook, 500);
  }
  new MutationObserver(function(records){
    records.forEach(function(record){
      for (var i=0;i<record.addedNodes.length;i++) {
        var node = record.addedNodes[i];
        if (node && node.matches && node.matches(SELECTORS)) hookVideo(node);
        if (node && node.querySelectorAll) {
          var videos = node.querySelectorAll(SELECTORS);
          for (var j=0;j<videos.length;j++) hookVideo(videos[j]);
        }
      }
    });
  }).observe(document.documentElement, { childList: true, subtree: true });
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', tryHook);
  else tryHook();
})();"#;

fn escape_js_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn capture_script(source_url: &str, request_id: &str) -> String {
    let src = escape_js_string(source_url);
    let req = escape_js_string(request_id);
    r#"(function(){
  var SOURCE = '__SOURCE__';
  var REQ = '__REQ__';
  function emit(event, payload){ try { window.__TAURI_INTERNALS__.invoke('plugin:event|emit', { event: event, payload: payload }); } catch(e){} }
  function escapeHtml(value){
    return String(value || '').replace(/[&<>"']/g, function(ch){
      return {'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[ch];
    });
  }
  function itemAnchor(item){
    var id = item.bv_id || item.bvid || '';
    if (!id) return '';
    return '<a href="/video/' + encodeURIComponent(id) + '">' + escapeHtml(item.title) + '</a>';
  }
  async function run(){
    var html = document.documentElement.outerHTML;
    var state = window.__INITIAL_STATE__ || {};
    var playlist = state.playlist || {};
    var pageItems = Array.isArray(state.resourceList) ? state.resourceList.slice() : [];
    var listTotal = Number(state.listTotal || (state.mediaListInfo && state.mediaListInfo.media_count) || pageItems.length);
    var url = new URL(SOURCE);
    var listMatch = url.pathname.match(/^\/list\/(ml)?(\d+)/);
    var isModernMediaList = Boolean(listMatch && listMatch[1] === 'ml');
    var playlistId = String(playlist.id || (listMatch && listMatch[2]) || '');
    var playlistType = Number(playlist.type || (isModernMediaList ? 3 : 1));
    var listTitle = String((state.mediaListInfo && state.mediaListInfo.title) || (playlist && playlist.title) || '');
    var isVideoPage = /\/video\//.test(new URL(SOURCE).pathname);
    if (isVideoPage) {
      // 分P 视频（多 P 单 BV，无 ugc_season）：每个 P 没有独立 BV，?p=N 是唯一能直接播该 P
      // 的稳定链接，即每首歌的"真实 url"。每个 ?p=N 当独立条目，播放=整页导航到 ?p=N，
      // B 站原生即播该 P，不挂钩 B 站分P SPA 状态机。合集（ugc_season）/单视频（pages≤1）
      // 不进此支路，fall-through 到下面已有 pod 逻辑。
      var vd = state.videoData || {};
      var pages = Array.isArray(vd.pages) ? vd.pages : [];
      if (!vd.ugc_season && pages.length > 1) {
        var bvid = String(vd.bvid || state.bvid || '');
        listTitle = String(vd.title || listTitle || '');
        html = pages.map(function(pg){
          var pn = Number(pg.page) || 1;
          var part = String(pg.part || ('P' + pn));
          return '<a href="/video/' + encodeURIComponent(bvid) + '/?p=' + pn + '">' + escapeHtml(part) + '</a>';
        }).join('');
        emit('bili://capture-html', { sourceUrl: SOURCE, requestId: REQ, listTitle: listTitle, html: html });
        return;
      }
      var podTitle = document.querySelector('.video-pod__header .title, .video-pod__head .title, .video-pod__header .left .title');
      if (podTitle) listTitle = (podTitle.textContent || '').trim();
      var podItems = Array.from(document.querySelectorAll('.video-pod__item[data-key]'));
      if (podItems.length) {
        html = podItems.map(function(node){
          var id = node.getAttribute('data-key') || '';
          var titleNode = node.querySelector('.title-txt, .title');
          var title = titleNode ? (titleNode.getAttribute('title') || titleNode.textContent || '').trim() : '';
          return '<a href="/video/' + encodeURIComponent(id) + '">' + escapeHtml(title) + '</a>';
        }).join('');
      }
      emit('bili://capture-html', { sourceUrl: SOURCE, requestId: REQ, listTitle: listTitle, html: html });
      return;
    }
    try {
      var cursorOid = url.searchParams.get('oid') || playlistId;
      var cursorBvid = url.searchParams.get('bvid') || '';
      var cursorStartKey = '';
      var fetched = [];
      var fetchedIds = new Set();
      var seenCursors = new Set();
      var hasMore = true;
      var pageCount = 0;
      var deadline = Date.now() + 17000;
      while (hasMore) {
        if (++pageCount > 1000) throw new Error('列表分页过多');
        var cursorKey = cursorStartKey || (cursorOid + '|' + cursorBvid);
        if (seenCursors.has(cursorKey)) throw new Error('分页游标未前进');
        seenCursors.add(cursorKey);
        var params = new URLSearchParams({
          type: String(playlistType), otype: '2', biz_id: playlistId,
          oid: cursorOid, bvid: cursorBvid, with_current: fetched.length === 0 ? 'true' : 'false',
          mobi_app: 'web', ps: '20', direction: 'false', sort_field: '1', tid: '0', desc: 'true'
        });
        if (cursorStartKey) params.set('start_key', cursorStartKey);
        var remaining = deadline - Date.now();
        if (remaining <= 0) throw new Error('完整列表解析超时');
        var controller = new AbortController();
        var requestTimer = setTimeout(function(){ controller.abort(); }, Math.min(8000, remaining));
        var response;
        try {
          response = await fetch('__BILI_MEDIALIST_API__?' + params.toString(), {
            credentials: 'include', signal: controller.signal
          });
        } finally {
          clearTimeout(requestTimer);
        }
        if (!response.ok) throw new Error('HTTP ' + response.status);
        var payload = await response.json();
        if (payload.code !== 0 || !payload.data) throw new Error(payload.message || '列表分页接口失败');
        var batch = Array.isArray(payload.data.media_list) ? payload.data.media_list : [];
        hasMore = Boolean(payload.data.has_more);
        if (batch.length === 0) {
          if (hasMore) throw new Error('列表分页返回空页');
          break;
        }
        batch.forEach(function(item){
          var itemId = String(item.bv_id || item.bvid || item.id || item.oid || '');
          if (itemId && !fetchedIds.has(itemId)) { fetchedIds.add(itemId); fetched.push(item); }
        });
        listTotal = Number(payload.data.total_count || listTotal);
        var nextStartKey = String(payload.data.next_start_key || '');
        if (nextStartKey) {
          cursorStartKey = nextStartKey;
          cursorOid = '';
          cursorBvid = '';
          continue;
        }
        var last = batch[batch.length - 1];
        cursorOid = String(last.id || last.oid || '');
        cursorBvid = String(last.bv_id || last.bvid || '');
      }
      pageItems = fetched;
    } catch(e) {
      emit('bili://capture-html', { sourceUrl: SOURCE, requestId: REQ, listTitle: listTitle, error: String(e && e.message ? e.message : e) });
      return;
    }
    html = pageItems.map(itemAnchor).join('');
    emit('bili://capture-html', { sourceUrl: SOURCE, requestId: REQ, listTitle: listTitle, html: html });
  }
  function wait(){
    if (document.readyState !== 'complete') { setTimeout(wait, 100); return; }
    var deadline = Date.now() + 8000;
    (function poll(){
      if (document.querySelector('a[href*="/video/"]') || Date.now() > deadline) run();
      else setTimeout(poll, 200);
    })();
  }
  wait();
})();"#
    .replace("__SOURCE__", &src)
    .replace("__REQ__", &req)
    .replace("__BILI_MEDIALIST_API__", BILI_MEDIALIST_API)
}

/// 刷新单个视频标题的注入脚本：在子 webview 当前页直接 fetch B 站单视频 view API
/// （同域、带 cookie），取 data.title 回传。**不导航、不打断当前播放**。
/// 仅当子 webview 不在 bilibili 域时，由 refresh_video_title 兜底导航首页后再 eval 此脚本。
/// 只读 API 返回的 title，不读媒体流地址或 cookie（合规）。
fn video_meta_script(bvid: &str, request_id: &str) -> String {
    let bv = escape_js_string(bvid);
    let req = escape_js_string(request_id);
    r#"(function(){
  var BVID = '__BVID__';
  var REQ = '__REQ__';
  function emit(event, payload){ try { window.__TAURI_INTERNALS__.invoke('plugin:event|emit', { event: event, payload: payload }); } catch(e){} }
  async function run(){
    try {
      var controller = new AbortController();
      var timer = setTimeout(function(){ controller.abort(); }, 8000);
      var response;
      try {
        response = await fetch('https://api.bilibili.com/x/web-interface/view?bvid=' + encodeURIComponent(BVID), {
          credentials: 'include', signal: controller.signal
        });
      } finally { clearTimeout(timer); }
      if (!response.ok) throw new Error('HTTP ' + response.status);
      var payload = await response.json();
      if (payload.code !== 0 || !payload.data) throw new Error(payload.message || '视频信息接口失败');
      var title = String((payload.data.title || '')).trim();
      if (!title) throw new Error('视频无标题');
      emit('bili://video-meta', { requestId: REQ, bvid: String(payload.data.bvid || BVID), title: title });
    } catch(e) {
      emit('bili://video-meta', { requestId: REQ, error: String(e && e.message ? e.message : e) });
    }
  }
  run();
})();"#
    .replace("__BVID__", &bv)
    .replace("__REQ__", &req)
}

fn playback_settings_script() -> &'static str {
    r#"(function(){
  var KEY = '__BILI_LIST_PLAYER_PLAYBACK_SETTINGS__';
  var labels = {
    autoNext: ['自动切集'],
    autoPlay: ['自动开播', '自动播放']
  };
  function textOf(node){
    var parent = node && node.parentElement;
    return ((node && (node.innerText || node.textContent || '')) + ' ' +
      (node && (node.getAttribute('aria-label') || node.getAttribute('title') || '')) + ' ' +
      (parent && (parent.innerText || parent.textContent || ''))).replace(/\s+/g, '');
  }
  function findControl(names){
    var nodes = document.querySelectorAll('input[type="checkbox"],[role="switch"],[role="checkbox"],button,[class*="setting"]');
    for (var i=0;i<nodes.length;i++) {
      var text = textOf(nodes[i]);
      for (var j=0;j<names.length;j++) if (text.indexOf(names[j]) >= 0) return nodes[i];
    }
    return null;
  }
  function isOn(node){
    if (!node) return null;
    if (typeof node.checked === 'boolean') return node.checked;
    var aria = node.getAttribute('aria-checked');
    if (aria === 'true' || aria === 'false') return aria === 'true';
    var cls = (' ' + (node.className || '') + ' ' + (node.parentElement && node.parentElement.className || '') + ' ').toLowerCase();
    if (/\b(active|on|checked|enabled|open)\b/.test(cls)) return true;
    if (/\b(off|unchecked|disabled|close|closed)\b/.test(cls)) return false;
    return null;
  }
  function clickIfNeeded(node, desired){
    var current = isOn(node);
    if (current !== null && current !== desired) node.click();
  }
  function readState(){
    try { return JSON.parse(sessionStorage.getItem(KEY) || 'null'); } catch(e) { return null; }
  }
  function writeState(state){
    try { sessionStorage.setItem(KEY, JSON.stringify(state)); } catch(e) {}
  }
  function disable(){
    var state = readState() || { captured: {}, original: {} };
    ['autoNext','autoPlay'].forEach(function(name){
      var node = findControl(labels[name]);
      if (!node) return;
      var current = isOn(node);
      if (!state.captured[name] && current !== null) {
        state.captured[name] = true;
        state.original[name] = current;
      }
      clickIfNeeded(node, false);
    });
    if (Object.keys(state.captured).length) writeState(state);
  }
  function restore(){
    var state = readState();
    if (!state) return;
    ['autoNext','autoPlay'].forEach(function(name){
      if (!state.captured[name]) return;
      var node = findControl(labels[name]);
      if (node) clickIfNeeded(node, state.original[name]);
    });
  }
  window.__BILI_LIST_PLAYER_RESTORE_SETTINGS__ = restore;
  if (!window.__BILI_LIST_PLAYER_SETTINGS_HOOKED__) {
    window.__BILI_LIST_PLAYER_SETTINGS_HOOKED__ = true;
    window.addEventListener('beforeunload', restore);
  }
  var deadline = Date.now() + 10000;
  (function poll(){
    disable();
    if (Date.now() < deadline) setTimeout(poll, 250);
  })();
})();"#
}

pub(crate) fn restore_playback_settings_script() -> &'static str {
    "window.__BILI_LIST_PLAYER_RESTORE_SETTINGS__ && window.__BILI_LIST_PLAYER_RESTORE_SETTINGS__();"
}

/// 播放控制脚本共用的 JS 辅助：轮询等待 B 站 SPA 异步创建的 <video>、带静音降级的播放。
/// Finished 后 800ms B 站播放器未必建好 <video>，故先轮询（每 250ms、上限 10s）再操作。
/// play() 被 WebView2 自动播放策略（NotAllowedError）拒绝时降级为静音播放，并注册一次性
/// 手势监听在用户首次交互时解除静音；__BILI_UNMUTE_PENDING__ 标志防多次 Play 堆叠监听。
const PLAY_CONTROL_HELPERS: &str = r#"(function(){
  // 事件上报：与 BRIDGE_INIT 的 emit 同实现。控制脚本独立 eval，不复用 BRIDGE_INIT 作用域。
  function emit(event, payload){
    try { window.__TAURI_INTERNALS__.invoke('plugin:event|emit', { event: event, payload: payload }); } catch(e){}
  }
  // hook 真 <video> 的事件（play/pause/ended/error/timeupdate）→ emit bili://video-event。
  // 与 BRIDGE_INIT 的 hookVideo 同逻辑，共用 __BILI_HOOKED__ 守卫互斥。关键修复：
  // BRIDGE_INIT 的 document 级 MutationObserver 抓不到 <bwp-video> 内（疑似 Shadow DOM）的
  // 真 <video> → 事件永不发到 Rust → 前端拿不到 duration（进度条 disabled）且 ended 不触发切歌。
  // 控制脚本的 pickVideo 能找到该 <video>，故在此 hook，确保事件上报可靠。
  // 只读 currentTime/duration（合规），不读媒体流地址或 cookie。
  function ensureHooked(v){
    if (!v || v.tagName !== 'VIDEO' || v.__BILI_HOOKED__) return;
    v.__BILI_HOOKED__ = true;
    ['play','pause','ended','error','timeupdate'].forEach(function(t){
      v.addEventListener(t, function(){
        var p = { type: t, itemId: window.location.pathname, positionSeconds: v.currentTime || 0 };
        if (isFinite(v.duration) && v.duration > 0) p.durationSeconds = v.duration;
        emit('bili://video-event', p);
      });
    });
    // DASH 流尾部未缓冲时原生 ended 可能不触发，用 timeupdate 兜底（与 BRIDGE_INIT 同）。
    v.addEventListener('timeupdate', function(){
      if (isFinite(v.duration) && v.duration > 0 && (v.duration - (v.currentTime || 0)) < 0.5 && !v.__BILI_ENDING__) {
        v.__BILI_ENDING__ = true;
        emit('bili://video-event', { type: 'ended', itemId: window.location.pathname, positionSeconds: v.currentTime || 0, durationSeconds: v.duration });
      }
    });
    v.addEventListener('play', function(){ v.__BILI_ENDING__ = false; });
  }
  // B 站可能用 <bwp-video> 自定义元素包裹真实 <video>，且 SPA 切歌瞬间会有多个 <video>
  // （旧播放器未销毁 + 新的在建）。querySelector 第一个未必是当前可见的那个 → 对它设
  // currentTime/play 会静默失败。故遍历取「可见且有尺寸、readyState>=1」的那个。
  function pickVideo(){
    var all = document.querySelectorAll('bwp-video, .bpx-player-container video, .bpx-player-video-wrap video, .bilibili-player video, video');
    var fallback = null;
    for (var i=0;i<all.length;i++){
      var v = all[i];
      // 只控制真正的 <video>（HTMLVideoElement）。<bwp-video> 是 B 站自定义元素，没有
      // HTMLMediaElement 的 play()/readyState/videoWidth/currentTime，操作它无法控制播放。
      // 早期页面 <bwp-video> 先于内部 <video> 出现，曾作为 fallback 被返回 → v.play() 抛错
      // 或无效 → 加载后不自动开播（用户却可手动按播放按钮开播，因为那时 <video> 已就绪）。
      // bwp-video 留在 selector 仅供 BRIDGE_INIT hook 事件，控制时一律跳过。
      if (!v || v.tagName !== 'VIDEO') continue;
      if (!fallback) fallback = v;
      // 优先可见（offsetParent 非 null 且有视频尺寸）且已就绪的元素。
      if (v.readyState >= 1 && v.videoWidth > 0 && v.offsetParent !== null) return v;
    }
    return fallback;
  }
  // B 站播放/暂停按钮（新版 bpx 与旧版 bilibili-player 两套选择器）。
  function pickPlayBtn(){
    return document.querySelector('.bpx-player-ctrl-play') ||
      document.querySelector('.bilibili-player-video-btn-start') ||
      document.querySelector('.bpx-player-video-btn-start') ||
      document.querySelector('[aria-label="播放"]') || document.querySelector('[aria-label="暂停"]');
  }
  function oncePlay(v){
    if (!v || typeof v.play !== 'function') return Promise.resolve();
    window.__BILI_LIST_PLAYER_PLAY_ALLOWANCE__ = (window.__BILI_LIST_PLAYER_PLAY_ALLOWANCE__||0)+1;
    return v.play().then(function(){}).catch(function(e){
      if (e && e.name === 'NotAllowedError') {
        v.muted = true;
        window.__BILI_LIST_PLAYER_PLAY_ALLOWANCE__ = (window.__BILI_LIST_PLAYER_PLAY_ALLOWANCE__||0)+1;
        v.play().catch(function(){});
        if (!window.__BILI_UNMUTE_PENDING__) {
          window.__BILI_UNMUTE_PENDING__ = true;
          function unmute(){ try { v.muted = false; } catch(_){} window.__BILI_UNMUTE_PENDING__ = false; window.removeEventListener('pointerdown', unmute, true); window.removeEventListener('keydown', unmute, true); }
          window.addEventListener('pointerdown', unmute, true);
          window.addEventListener('keydown', unmute, true);
        }
      }
    });
  }
  // play() 可能静默成功但视频未真正开播（B 站播放器遮罩/占位 video），故主动播放并复核重试。
  // 关键：B 站「自动开播」被我方禁用后，页面加载完 <video> 未必有流（readyState=0、无 src）→
  // v.play() 以非 NotAllowedError 失败、静音降级不触发 → 卡住。重试按 readyState 分诊：
  //   readyState<1（必为暂停、无流）：点 B 站自己的播放按钮触发它 fetch playurl 并设 src
  //     （src 设置发生在 B 站内部 play() 之前；我方守卫会拦截 B 站那次 play()，但 src 已就绪）。
  //     仅在 readyState<1 时点按钮，避免「播放中点按钮」toggle 成暂停的竞态。
  //   readyState>=1（已有流）：oncePlay 即可（守卫 allowance+1；浏览器拦则静音降级）。
  // 每轮重新 pickVideo：bpx-player SPA 切歌可能换 <video> 元素，旧引用会失效。
  function playV(v){
    function bootstrap(cur){
      if (!cur || typeof cur.play !== 'function') return;
      ensureHooked(cur); // 确保找到的 <video> 已 hook 事件，Rust/前端才能收到 duration/ended。
      if (cur.readyState < 1) {
        var btn = pickPlayBtn();
        if (btn) { try { btn.click(); } catch(_){} }
      }
      oncePlay(cur);
    }
    bootstrap(v);
    var tries = 0;
    setTimeout(function check(){
      if (tries++ > 12) return; // ~12×800ms≈9.6s
      var cur = pickVideo() || v;
      if (!cur || typeof cur.play !== 'function') { setTimeout(check, 800); return; }
      ensureHooked(cur); // SPA 切歌可能换 <video> 元素，重 hook 新的。
      if (cur.paused) {
        bootstrap(cur);
        setTimeout(check, 800);
      } else if ((cur.currentTime || 0) <= 0 && cur.readyState < 3) {
        // 播放中但卡在 0（缓冲中）：幂等 play，不点按钮以免 toggle 成暂停。
        oncePlay(cur);
        setTimeout(check, 800);
      }
    }, 800);
  }
  function pauseV(v){
    try { v.pause(); } catch(_){}
    // 直接 pause 失败时点 B 站暂停按钮兜底。
    var btn = pickPlayBtn();
    if (btn && !v.paused) { try { btn.click(); } catch(_){} }
  }
  function poll(fn){ var deadline = Date.now() + 10000; (function step(){ var v = pickVideo(); if (v) { ensureHooked(v); fn(v); return; } if (Date.now() < deadline) setTimeout(step, 250); })(); }
  // seek：用 readyState>=2（HAVE_CURRENT_DATA）确保 duration 已就绪，否则 Math.min(max(p,0),NaN)
  // 得 NaN 会让 currentTime=NaN 静默失败（「点了没反应」根因）。未就绪则等 loadedmetadata 再 seek，
  // 并在 500ms 后复核：若位置未到目标则重试一次（DASH 流缓冲延迟兜底）。
  // onDone：可选完成回调，在首次 doSeek 执行后触发一次（复核重试不重复触发）。seek_and_play
  // 借此让 play 在 seek 到位后再跑，避免 readyState<2 时 play 先以旧位置开播、seek 随后才拉回 0。
  function seekTo(v, p, onDone){
    var done = false;
    var fire = function(){ if (!done) { done = true; if (typeof onDone === 'function') onDone(); } };
    var doSeek = function(){
      if (isFinite(v.duration) && v.duration > 0) p = Math.min(Math.max(p, 0), v.duration);
      try { v.currentTime = p; } catch(_){}
      fire();
    };
    if (v.readyState >= 2) doSeek();
    else v.addEventListener('loadedmetadata', doSeek, { once: true });
    setTimeout(function(){
      // 500ms 复核：若位置偏差大且已就绪则重 seek 一次；无论就绪与否，若 onDone 仍未触发
      // （loadedmetadata 迟迟不来、readyState 卡 <2）则兜底触发 onDone，避免 play 永不执行
      // （双击切歌后不播放的回归根因）。
      if (Math.abs((v.currentTime || 0) - p) > 1.5) {
        if (v.readyState >= 2) doSeek();
      }
      fire();
    }, 500);
  }
  window.__BILI_CTRL__ = { play: playV, pause: pauseV, poll: poll, seekTo: seekTo, pickVideo: pickVideo, pickPlayBtn: pickPlayBtn };
})();"#;

fn control_script(cmd: &PlaybackCommandDto) -> String {
    match cmd {
        PlaybackCommandDto::Play => format!("{helpers}window.__BILI_CTRL__.poll(function(v){{window.__BILI_CTRL__.play(v);}});", helpers = PLAY_CONTROL_HELPERS),
        PlaybackCommandDto::Pause => format!(
            r#"{helpers}window.__BILI_CTRL__.poll(function(v){{window.__BILI_CTRL__.pause(v);}});"#,
            helpers = PLAY_CONTROL_HELPERS
        ),
        PlaybackCommandDto::Seek { position_seconds } => format!(
            r#"{helpers}window.__BILI_CTRL__.poll(function(v){{window.__BILI_CTRL__.seekTo(v,{p});}});"#,
            helpers = PLAY_CONTROL_HELPERS,
            p = position_seconds
        ),
        PlaybackCommandDto::Next | PlaybackCommandDto::Previous | PlaybackCommandDto::Load { .. } => {
            String::new()
        }
    }
}

/// Load 流程在目标页 Finished 后调用：轮询等到 <video> 出现后先 seek 到续播位，
/// 再按 should_play 播放（带静音降级）。play 作为 seekTo 的完成回调，在 seek 到位后
/// 才触发——避免 readyState<2 时 play 先以 B 站初始/历史位置开播、seek 随后才拉回 0
/// （「不从 0 开始」根因）。
fn seek_and_play_control_script(position_seconds: f64, should_play: bool) -> String {
    let play = if should_play {
        "window.__BILI_CTRL__.play(v);"
    } else {
        ""
    };
    format!(
        r#"{helpers}window.__BILI_CTRL__.poll(function(v){{window.__BILI_CTRL__.seekTo(v,{p},function(){{{play}}});}});"#,
        helpers = PLAY_CONTROL_HELPERS,
        p = position_seconds,
        play = play
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedVideoEvent {
    pub event_type: String,
    pub item_id: String,
    pub position_seconds: f64,
    pub duration_seconds: Option<f64>,
}

const KNOWN_VIDEO_EVENTS: [&str; 5] = ["play", "pause", "ended", "error", "timeupdate"];

fn validate_video_event_payload(value: &serde_json::Value) -> Option<ParsedVideoEvent> {
    let obj = value.as_object()?;
    let event_type = obj.get("type")?.as_str()?.to_string();
    if !KNOWN_VIDEO_EVENTS.contains(&event_type.as_str()) {
        return None;
    }
    let item_id = obj.get("itemId")?.as_str()?.to_string();
    let position_seconds = obj.get("positionSeconds")?.as_f64()?;
    let duration_seconds = obj.get("durationSeconds").and_then(|v| v.as_f64());
    Some(ParsedVideoEvent {
        event_type,
        item_id,
        position_seconds,
        duration_seconds,
    })
}

#[derive(Default)]
pub struct CaptureState {
    pub current_url: Mutex<Option<String>>,
    pub pending_capture: Mutex<Option<(String, String)>>, // (source_url, request_id)
    pub pending_seek: Mutex<Option<f64>>,
    pub pending_play: Mutex<bool>,
    pub pending_playback_url: Mutex<Option<String>>,
    /// 刷新单视频标题的待执行意图 (bvid, request_id)。子 webview 不在 bilibili 域时，
    /// refresh_video_title 先导航首页，Finished 后由此消费再 eval video_meta_script。
    pub pending_meta: Mutex<Option<(String, String)>>,
    /// 前端最近上报的播放区矩形（逻辑像素 x,y,w,h）。子 webview 尚未创建时缓存，
    /// `ensure_bili_webview` 创建后立即套用，避免 (0,0) 全窗闪现。
    pub last_bounds: Mutex<Option<(f64, f64, f64, f64)>>,
}

/// 转发到前端的播放事件（镜像 TS `PlaybackEvent` 联合类型）。
/// `tag = "type"` → `{"type":"started",...}`；字段直接用 camelCase 命名以对齐 TS DTO
///（enum 级 `rename_all` 只重命名变体名，不重命名变体内字段，故字段名直接取 camelCase）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
#[allow(non_snake_case)]
enum PlaybackEventEmit {
    Started {
        itemId: String,
        positionSeconds: f64,
    },
    Paused {
        itemId: String,
        positionSeconds: f64,
    },
    Ended {
        itemId: String,
        positionSeconds: f64,
    },
    Progress {
        itemId: String,
        positionSeconds: f64,
        durationSeconds: f64,
    },
    Error {
        itemId: String,
        message: String,
    },
}

/// 把注入脚本上报的原始视频事件（play/pause/ended/error/timeupdate）
/// 规范化为前端 `PlaybackEvent` 语义（started/paused/ended/error/progress）。
/// error 事件无原生消息字段，统一给中文兜底文案。
fn map_video_event(parsed: ParsedVideoEvent, item_id: String) -> Option<PlaybackEventEmit> {
    match parsed.event_type.as_str() {
        "play" => Some(PlaybackEventEmit::Started {
            itemId: item_id,
            positionSeconds: parsed.position_seconds,
        }),
        "pause" => Some(PlaybackEventEmit::Paused {
            itemId: item_id,
            positionSeconds: parsed.position_seconds,
        }),
        "ended" => Some(PlaybackEventEmit::Ended {
            itemId: item_id,
            positionSeconds: parsed.position_seconds,
        }),
        "timeupdate" => Some(PlaybackEventEmit::Progress {
            itemId: item_id,
            positionSeconds: parsed.position_seconds,
            durationSeconds: parsed.duration_seconds.unwrap_or(0.0),
        }),
        "error" => Some(PlaybackEventEmit::Error {
            itemId: item_id,
            message: "视频播放失败".to_string(),
        }),
        _ => None,
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ParseResultPayload {
    request_id: String,
    source_url: String,
    items: Vec<crate::model::ParsedItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    list_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PageStatePayload {
    url: String,
    login_state: String, // "ready" | "verification-required"
}

pub fn register(app: &AppHandle) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let app_v = app.clone();
    app.listen("bili://video-event", move |event| {
        let payload_str = event.payload();
        let value: serde_json::Value = match serde_json::from_str(payload_str) {
            Ok(v) => v,
            Err(_) => return,
        };
        let Some(parsed) = validate_video_event_payload(&value) else {
            return;
        };
        let item_id = parser::normalize_video_id(&parsed.item_id).unwrap_or_else(|| parsed.item_id.clone());
        if let Some(typed) = map_video_event(parsed, item_id) {
            let _ = app_v.emit_to("main", "bilibili://playback-event", typed);
        }
    });

    let app_c = app.clone();
    app.listen("bili://capture-html", move |event| {
        let value: serde_json::Value = match serde_json::from_str(event.payload()) {
            Ok(v) => v,
            Err(_) => return,
        };
        let Some(obj) = value.as_object() else {
            return;
        };
        let Some(source_url) = obj.get("sourceUrl").and_then(|v| v.as_str()) else {
            return;
        };
        let Some(request_id) = obj.get("requestId").and_then(|v| v.as_str()) else {
            return;
        };
        let list_title = obj.get("listTitle").and_then(|v| v.as_str()).map(|s| s.to_string());
        if parser::validate_list_url(source_url).is_err() {
            return;
        }
        if let Some(error) = obj.get("error").and_then(|v| v.as_str()) {
            let typed = ParseResultPayload {
                request_id: request_id.to_string(),
                source_url: source_url.to_string(),
                items: Vec::new(),
                list_title,
                error: Some(error.to_string()),
            };
            let _ = app_c.emit_to("main", "bilibili://parse-result", typed);
            return;
        }
        let Some(html) = obj.get("html").and_then(|v| v.as_str()) else {
            return;
        };
        if html.len() > MAX_CAPTURE_HTML {
            return;
        }
        let items = parser::parse_list_html(source_url, html);
        let typed = ParseResultPayload {
            request_id: request_id.to_string(),
            source_url: source_url.to_string(),
            items,
            list_title,
            error: None,
        };
        let _ = app_c.emit_to("main", "bilibili://parse-result", typed);
    });

    // 刷新单视频标题：video_meta_script emit 的 {requestId,bvid,title?|error?} 透传到主 webview。
    let app_m = app.clone();
    app.listen("bili://video-meta", move |event| {
        let value: serde_json::Value = match serde_json::from_str(event.payload()) {
            Ok(v) => v,
            Err(_) => return,
        };
        let _ = app_m.emit_to("main", "bilibili://video-meta", value);
    });

    let app_p = app.clone();
    app.listen("bili://page-loaded", move |event| {
        let value: serde_json::Value = match serde_json::from_str(event.payload()) {
            Ok(v) => v,
            Err(_) => return,
        };
        let Some(url) = value.get("url").and_then(|v| v.as_str()) else {
            return;
        };
        let _ = app_p.emit_to(
            "main",
            "bilibili://page-state",
            PageStatePayload {
                url: url.to_string(),
                login_state: page_access_state(url).to_string(),
            },
        );
    });

    Ok(())
}
/// 被 async 命令调用——源码（webview/mod.rs:290/331）明示 `add_child` 在 async
/// 命令里不会死锁。已存在则返回现有子 webview。
fn ensure_bili_webview(app: &AppHandle) -> Result<Webview, String> {
    if let Some(wv) = app.get_webview(BILI_LABEL) {
        return Ok(wv);
    }
    // add_child 在 Window 上（feature="unstable"），故取 Window 而非 WebviewWindow。
    let main_window = app
        .get_window("main")
        .ok_or("主窗口未找到".to_string())?;
    let parsed: Url = BILI_HOME
        .parse()
        .map_err(|e: url::ParseError| e.to_string())?;
    // 持久化数据目录：WebView2 在此存 cookie，登一次重启免登。
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("bili-webview");
    let builder = WebviewBuilder::new(BILI_LABEL, WebviewUrl::External(parsed))
        .data_directory(data_dir)
        .on_navigation(|u: &Url| is_allowed_bili_host(u.host_str().unwrap_or("")))
        .initialization_script(BRIDGE_INIT)
        .on_page_load(|webview: Webview, payload| {
            if payload.event() != PageLoadEvent::Finished {
                return;
            }
            let url = payload.url().to_string();
            let state = webview.state::<CaptureState>();
            *state.current_url.lock().unwrap() = Some(url.clone());
            let pending = state.pending_capture.lock().unwrap().take();
            if let Some((src, req)) = pending {
                if same_list_page(&src, &url) {
                    let _ = webview.eval(&capture_script(&src, &req));
                } else {
                    // 不匹配（about:blank / 中转风控页）→ 放回 pending，等目标列表页 Finished 再注入。
                    // 否则 about:blank 的 Finished 会消耗掉 pending，目标页加载完反而无脚本注入。
                    *state.pending_capture.lock().unwrap() = Some((src, req));
                }
            }
            // 刷新标题兜底：refresh_video_title 在子 webview 不在 bilibili 域时导航首页并
            // 暂存 (bvid, request_id)；首页 Finished（bilibili 域）即可 fetch 同域 view API，
            // 不依赖具体页面。非 bilibili 域（about:blank/中转）则放回等下一页。
            let pending_meta = state.pending_meta.lock().unwrap().take();
            if let Some((bv, req)) = pending_meta {
                let host = Url::parse(&url).ok().and_then(|u| u.host_str().map(|h| h.to_string()));
                if host.as_deref().is_some_and(is_allowed_bili_host) {
                    let _ = webview.eval(&video_meta_script(&bv, &req));
                } else {
                    *state.pending_meta.lock().unwrap() = Some((bv, req));
                }
            }
            // 播放意图只在目标页 Finished 时消费。中间页（建子 webview 时的 BILI_HOME、
            // about:blank、风控中转）playback_matches=false → 一律不动 pending_*，
            // 避免首页偷走 pending_play 导致视频页加载完反而不播放。
            let playback_matches = state
                .pending_playback_url
                .lock()
                .unwrap()
                .as_deref()
                .is_some_and(|target| same_list_page(target, &url));
            if playback_matches {
                *state.pending_playback_url.lock().unwrap() = None;
                let pending_seek = state.pending_seek.lock().unwrap().take();
                let should_play = {
                    let mut pending_play = state.pending_play.lock().unwrap();
                    let value = *pending_play;
                    *pending_play = false;
                    value
                };
                if let Some(pos) = pending_seek {
                    let wv = webview.clone();
                    // Finished 后 B 站 SPA 未必已建好 <video>，故延一拍再用轮询脚本
                    // 等待 <video> 出现后再 seek + play（合并成一次 eval，避免两次独立轮询竞争）。
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(800));
                        let _ = wv.eval(&seek_and_play_control_script(pos, should_play));
                    });
                } else if should_play {
                    let _ = webview.eval(&control_script(&PlaybackCommandDto::Play));
                }
            }
            let _ = webview.eval(playback_settings_script());
        });
    // add_child 在 async 命令（工作线程）里调用，不死锁（webview/mod.rs:290/331）。
    // 初次以 (0,0)+主窗口尺寸创建，随后由前端 set_bili_webview_bounds 校准到播放区。
    let wv = main_window
        .add_child(
            builder,
            LogicalPosition::new(0.0, 0.0),
            main_window.inner_size().map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    // 创建后先隐藏，待前端上报播放区 bounds 再 show，避免 (0,0) 全窗闪现。
    let _ = wv.hide();
    // 套用前端已上报的播放区矩形：若有有效矩形则定位+显示，使子 webview 落在播放区而非全窗。
    let bounds = app
        .state::<CaptureState>()
        .last_bounds
        .lock()
        .unwrap()
        .clone();
    if let Some((x, y, w, h)) = bounds {
        if w > 0.0 && h > 0.0 {
            let _ = wv.set_position(LogicalPosition::new(x, y));
            let _ = wv.set_size(LogicalSize::new(w, h));
            let _ = wv.show();
        }
    }
    Ok(wv)
}

#[tauri::command]
pub async fn open_bilibili_webview(app: AppHandle, url: Option<String>) -> Result<(), String> {
    let wv = ensure_bili_webview(&app)?;
    if let Some(u) = &url {
        let parsed: Url = u.parse().map_err(|e: url::ParseError| e.to_string())?;
        if !is_allowed_bili_host(parsed.host_str().unwrap_or("")) {
            return Err("仅允许 Bilibili 站内导航".into());
        }
        wv.navigate(parsed).map_err(|e| e.to_string())?;
    }
    wv.show().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn navigate_bilibili_webview(app: AppHandle, url: String) -> Result<(), String> {
    let parsed: Url = url.parse().map_err(|e: url::ParseError| e.to_string())?;
    if !is_allowed_bili_host(parsed.host_str().unwrap_or("")) {
        return Err("仅允许 Bilibili 站内导航".into());
    }
    // 子 webview 尚未创建时自动创建并导航到目标 URL，保证导入/播放流程可独立调用。
    // 已存在则导航。可见性由 set_bili_webview_bounds 按 webviewVisibleRef 统一控制，
    // navigate 不自行 show——导入时 webview 须隐藏（后台抓取），否则原生层遮挡列表 UI。
    match app.get_webview(BILI_LABEL) {
        Some(wv) => {
            wv.navigate(parsed).map_err(|e| e.to_string())?;
        }
        None => {
            let wv = ensure_bili_webview(&app)?;
            wv.navigate(parsed).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn capture_list_html(
    app: AppHandle,
    state: State<'_, CaptureState>,
    source_url: String,
    request_id: String,
) -> Result<(), String> {
    parser::validate_list_url(&source_url).map_err(|e: ParseError| e.to_string())?;
    let parsed: Url = source_url
        .parse()
        .map_err(|e: url::ParseError| e.to_string())?;
    let current = state.current_url.lock().unwrap().clone();
    if current.as_deref().is_some_and(|c| same_list_page(c, &source_url)) {
        let wv = app
            .get_webview(BILI_LABEL)
            .ok_or("Bilibili WebView 未打开".to_string())?;
        wv.eval(&capture_script(&source_url, &request_id))
            .map_err(|e| e.to_string())?;
    } else {
        *state.pending_capture.lock().unwrap() = Some((source_url.clone(), request_id));
        match app.get_webview(BILI_LABEL) {
            Some(wv) => {
                wv.navigate(parsed).map_err(|e| e.to_string())?;
            }
            None => {
                let wv = ensure_bili_webview(&app)?;
                wv.navigate(parsed).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// 刷新单个视频标题：在子 webview 当前页直接 fetch view API 取最新 title，不导航、
/// 不打断当前播放。仅当子 webview 不存在或不在 bilibili 域时，兜底导航首页再 fetch。
#[tauri::command]
pub async fn refresh_video_title(
    app: AppHandle,
    state: State<'_, CaptureState>,
    bvid: String,
    request_id: String,
) -> Result<(), String> {
    let bv = parser::normalize_video_id(&bvid)
        .ok_or_else(|| "无效的视频 id".to_string())?;
    let current = state.current_url.lock().unwrap().clone();
    let on_bili = current
        .as_deref()
        .and_then(|u| Url::parse(u).ok())
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .is_some_and(|h| is_allowed_bili_host(&h));
    match app.get_webview(BILI_LABEL) {
        Some(wv) if on_bili => {
            // 当前在 bilibili 域：直接 eval fetch 脚本，不导航、不打断播放。
            wv.eval(&video_meta_script(&bv, &request_id))
                .map_err(|e| e.to_string())?;
        }
        _ => {
            // 子 webview 不存在或不在 bilibili 域：导航首页（轻量、不播视频），Finished 后兜底 fetch。
            *state.pending_meta.lock().unwrap() = Some((bv, request_id));
            let parsed: Url = BILI_HOME.parse().map_err(|e: url::ParseError| e.to_string())?;
            match app.get_webview(BILI_LABEL) {
                Some(wv) => {
                    wv.navigate(parsed).map_err(|e| e.to_string())?;
                }
                None => {
                    let wv = ensure_bili_webview(&app)?;
                    wv.navigate(parsed).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn send_playback_command(
    app: AppHandle,
    state: State<'_, CaptureState>,
    command: PlaybackCommandDto,
) -> Result<(), String> {
    match &command {
        PlaybackCommandDto::Load {
            url,
            position_seconds,
        } => {
            let parsed: Url = url.parse().map_err(|e: url::ParseError| e.to_string())?;
            if !is_allowed_bili_host(parsed.host_str().unwrap_or("")) {
                return Err("仅允许 Bilibili 站内导航".into());
            }
            *state.pending_seek.lock().unwrap() = Some(*position_seconds);
            // Load 默认就要播放（loadAndPlay 总是 load+play）。pending_play 初值置 true，
            // 即使前端 Play 命令因竞态晚于目标页 Finished 到达，Finished 时也能 should_play=true 自动开播。
            *state.pending_play.lock().unwrap() = true;
            *state.pending_playback_url.lock().unwrap() = Some(url.clone());
            match app.get_webview(BILI_LABEL) {
                Some(wv) => {
                    let _ = wv.eval(&control_script(&PlaybackCommandDto::Pause));
                    wv.navigate(parsed).map_err(|e| e.to_string())?;
                }
                None => {
                    let wv = ensure_bili_webview(&app)?;
                    wv.navigate(parsed).map_err(|e| e.to_string())?;
                }
            }
        }
        PlaybackCommandDto::Play => {
            if state.pending_seek.lock().unwrap().is_some() {
                *state.pending_play.lock().unwrap() = true;
                return Ok(());
            }
            let wv = app
                .get_webview(BILI_LABEL)
                .ok_or("Bilibili WebView 未打开".to_string())?;
            wv.eval(&control_script(&command))
                .map_err(|e| e.to_string())?;
        }
        PlaybackCommandDto::Pause => {
            // 防御：若上次 Load 的目标页 Finished 一直没匹配（导航失败/被风控跳转），
            // pending_seek 会残留，导致后续 Play 按钮被上面的 early-return 永久卡住。
            // 用户显式暂停即放弃那次未完成的加载意图，清掉以免卡死。
            if state.pending_playback_url.lock().unwrap().is_some() {
                *state.pending_seek.lock().unwrap() = None;
                *state.pending_play.lock().unwrap() = false;
                *state.pending_playback_url.lock().unwrap() = None;
            }
            let wv = app
                .get_webview(BILI_LABEL)
                .ok_or("Bilibili WebView 未打开".to_string())?;
            wv.eval(&control_script(&command))
                .map_err(|e| e.to_string())?;
        }
        PlaybackCommandDto::Seek { .. } => {
            let wv = app
                .get_webview(BILI_LABEL)
                .ok_or("Bilibili WebView 未打开".to_string())?;
            wv.eval(&control_script(&command))
                .map_err(|e| e.to_string())?;
        }
        PlaybackCommandDto::Next | PlaybackCommandDto::Previous => { /* no-op：前端推进队列后发 load */ }
    }
    Ok(())
}

/// 嵌入式不销毁子 webview，隐藏即可；下次 show 复用登录态与已加载页面。
#[tauri::command]
pub async fn close_bilibili_webview(app: AppHandle) -> Result<(), String> {
    if let Some(wv) = app.get_webview(BILI_LABEL) {
        wv.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 前端测得播放区 DOM 矩形（CSS 逻辑像素）后上报，Rust 校准子 webview 位置/尺寸。
/// width/height 为 0（播放区折叠，如窄窗口 .player{display:none}）则隐藏子 webview。
/// 矩形始终缓存：子 webview 尚未创建时仅缓存，`ensure_bili_webview` 创建时套用。
#[tauri::command]
pub async fn set_bili_webview_bounds(
    app: AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    {
        let state = app.state::<CaptureState>();
        *state.last_bounds.lock().unwrap() = Some((x, y, width, height));
    }
    if let Some(wv) = app.get_webview(BILI_LABEL) {
        wv.set_position(LogicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
        wv.set_size(LogicalSize::new(width, height))
            .map_err(|e| e.to_string())?;
        if width <= 0.0 || height <= 0.0 {
            wv.hide().map_err(|e| e.to_string())?;
        } else {
            wv.show().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PlaybackCommandDto;
    use serde_json::json;

    #[test]
    fn allows_bili_hosts() {
        assert!(is_allowed_bili_host("www.bilibili.com"));
        assert!(is_allowed_bili_host("bilibili.com"));
        assert!(is_allowed_bili_host("passport.bilibili.com"));
    }

    #[test]
    fn page_access_state_marks_public_bilibili_page_ready() {
        assert_eq!(
            page_access_state("https://www.bilibili.com/list/1"),
            "ready"
        );
    }

    #[test]
    fn page_access_state_marks_passport_page_verification_required() {
        assert_eq!(
            page_access_state("https://passport.bilibili.com/login"),
            "verification-required"
        );
    }

    #[test]
    fn blocks_non_bili() {
        assert!(!is_allowed_bili_host("example.com"));
        assert!(!is_allowed_bili_host("evil.bilibili.com.evil.com"));
        assert!(!is_allowed_bili_host(""));
    }

    #[test]
    fn same_list_page_ignores_query() {
        // B 站规范化掉 oid/bvid query 时，host+path 仍应判定为同一列表页。
        assert!(same_list_page(
            "https://www.bilibili.com/list/12853451?oid=1&bvid=BV1",
            "https://www.bilibili.com/list/12853451"
        ));
        assert!(same_list_page(
            "https://www.bilibili.com/list/12853451",
            "https://www.bilibili.com/list/12853451?oid=1&bvid=BV1"
        ));
        // 同一 URL 显然匹配。
        assert!(same_list_page(
            "https://www.bilibili.com/list/12853451?oid=1",
            "https://www.bilibili.com/list/12853451?oid=1"
        ));
        // 不同列表（path 不同）不匹配。
        assert!(!same_list_page(
            "https://www.bilibili.com/list/12853451",
            "https://www.bilibili.com/list/999"
        ));
        // 不同 host 不匹配。
        assert!(!same_list_page(
            "https://www.bilibili.com/list/1",
            "https://m.bilibili.com/list/1"
        ));
    }

    #[test]
    fn same_list_page_ignores_trailing_slash() {
        // B 站把视频页规范化为带尾斜杠，而解析器生成的项目 URL 无尾斜杠；
        // 忽略尾斜杠后播放意图才能在视频页 Finished 时匹配上。
        assert!(same_list_page(
            "https://www.bilibili.com/video/BV1",
            "https://www.bilibili.com/video/BV1/"
        ));
        assert!(same_list_page(
            "https://www.bilibili.com/video/BV1/",
            "https://www.bilibili.com/video/BV1"
        ));
        // 根路径的尾斜杠保留，仍匹配。
        assert!(same_list_page(
            "https://www.bilibili.com",
            "https://www.bilibili.com/"
        ));
        // 列表页同样忽略尾斜杠。
        assert!(same_list_page(
            "https://www.bilibili.com/list/12853451",
            "https://www.bilibili.com/list/12853451/"
        ));
    }

    #[test]
    fn capture_script_contains_payload_markers() {
        let s = capture_script("https://www.bilibili.com/list/1", "req-42");
        assert!(s.contains("https://www.bilibili.com/list/1"));
        assert!(s.contains("req-42"));
        assert!(s.contains("outerHTML"));
        assert!(s.contains("plugin:event|emit"));
    }

    #[test]
    fn capture_script_forbidden_literals() {
        let s = capture_script("https://www.bilibili.com/list/1", "req-42");
        assert!(!s.contains(".src"), "must not read media src");
        assert!(!s.contains("currentSrc"));
        assert!(!s.contains("mediaUrl"));
        assert!(!s.contains("download"));
    }

    #[test]
    fn video_meta_script_reads_only_title_and_avoids_forbidden_literals() {
        // video_meta_script 注入到远程 B 站页，同样受合规约束：只读 view API 返回的 title，
        // 绝不读媒体流地址或 cookie。审计禁字面量（与 capture_script 同标准）。
        let s = video_meta_script("BV1xx", "req-42");
        assert!(s.contains("x/web-interface/view"), "fetches single-video view API");
        assert!(s.contains("data.title"), "reads only the title field");
        assert!(!s.contains(".src"), "must not read media src");
        assert!(!s.contains("currentSrc"));
        assert!(!s.contains("mediaUrl"));
        assert!(!s.contains("download"));
        assert!(!s.contains("cookie"));
    }

    #[test]
    fn capture_script_loads_all_paginated_list_items() {
        let s = capture_script("https://www.bilibili.com/list/1", "req-42");
        assert!(s.contains("resourceList"));
        assert!(s.contains("listTotal"));
        assert!(s.contains("x/v2/medialist/resource/list"));
        assert!(s.contains("ps: '20'"));
        assert!(s.contains("pageItems"));
    }

    #[test]
    fn capture_script_supports_modern_media_list_urls() {
        let s = capture_script(
            "https://www.bilibili.com/list/ml3686407954?oid=488290436&bvid=BV1gN411U7kN",
            "req-42",
        );
        assert!(s.contains("pathname.match(/^\\/list\\/(ml)?(\\d+)/)"));
        assert!(s.contains("isModernMediaList"));
        assert!(s.contains("(isModernMediaList ? 3 : 1)"));
        assert!(s.contains("url.searchParams.get('oid')"));
        assert!(s.contains("url.searchParams.get('bvid')"));
    }

    #[test]
    fn capture_script_uses_api_next_start_key_for_pagination() {
        let s = capture_script("https://www.bilibili.com/list/ml3686407954", "req-42");
        assert!(s.contains("next_start_key"));
        assert!(s.contains("payload.data.next_start_key"));
        assert!(s.contains("start_key"));
    }

    #[test]
    fn capture_script_prefers_api_titles_over_dom_placeholders() {
        let s = capture_script("https://www.bilibili.com/list/1", "req-42");
        assert!(s.contains("html = pageItems.map(itemAnchor).join('')"));
        assert!(!s.contains("html = pageItems.map(itemAnchor).join('') + html"));
        assert!(s.contains("escapeHtml(item.title)"));
    }

    #[test]
    fn capture_script_extracts_video_page_collection_items() {
        let s = capture_script(
            "https://www.bilibili.com/video/BV1zK4y1F7NP/?spm_id_from=333.1387.0.0",
            "req-42",
        );
        assert!(s.contains("video-pod__item[data-key]"));
        assert!(s.contains("data-key"));
        assert!(s.contains("podTitle"));
        assert!(s.contains("isVideoPage"));
    }

    #[test]
    fn capture_script_emits_multipage_p_items() {
        // 分P 视频（无 ugc_season 且 pages>1）：每个 P 生成一条 ?p=N 独立条目。
        let s = capture_script(
            "https://www.bilibili.com/video/BV1NF411F7D3/?spm_id_from=333.1387.favlist",
            "req-42",
        );
        assert!(s.contains("vd.pages"), "读取 videoData.pages");
        assert!(s.contains("!vd.ugc_season"), "仅分P（非合集）走此支路");
        assert!(s.contains("pages.length > 1"), "多 P 才进分P 支路");
        assert!(s.contains("/?p='"), "每 P 链接带 ?p=");
        assert!(s.contains("encodeURIComponent(bvid)"), "bvid 取自 state");
        assert!(s.contains("pg.part"), "标题取自 page.part");
    }

    #[test]
    fn capture_script_rejects_partial_or_stalled_pagination() {
        let s = capture_script("https://www.bilibili.com/list/1", "req-42");
        assert!(s.contains("AbortController"));
        assert!(s.contains("seenCursors"));
        assert!(s.contains("分页游标未前进"));
        assert!(s.contains("error: String(e"));
        assert!(!s.contains("catch(e) {}"));
        assert!(!s.contains("fetched.length < listTotal"));
    }

    #[test]
    fn control_script_play() {
        let script = control_script(&PlaybackCommandDto::Play);
        // 轮询等待 <video> 出现后再播，带 NotAllowedError 静音降级、手势解静音与 B 站播放按钮兜底。
        assert!(script.contains("__BILI_CTRL__.poll"));
        assert!(script.contains("v.play()"));
        assert!(script.contains("__BILI_LIST_PLAYER_PLAY_ALLOWANCE__"));
        assert!(script.contains("NotAllowedError"));
        assert!(script.contains("v.muted"));
        assert!(script.contains("pickPlayBtn"));
    }

    #[test]
    fn control_script_pause() {
        let script = control_script(&PlaybackCommandDto::Pause);
        // 直接 v.pause()，失败时点 B 站暂停按钮兜底。
        assert!(script.contains("__BILI_CTRL__.pause"));
        assert!(script.contains("v.pause()"));
    }

    #[test]
    fn control_script_seek() {
        let script = control_script(&PlaybackCommandDto::Seek {
            position_seconds: 1.5
        });
        // 轮询等待 <video>，命中后用 seekTo（readyState>=2 + loadedmetadata 兜底 + 500ms 复核）。
        assert!(script.contains("__BILI_CTRL__.poll"));
        assert!(script.contains("__BILI_CTRL__.seekTo(v,1.5)"));
        assert!(script.contains("readyState >= 2"));
        assert!(script.contains("loadedmetadata"));
        assert!(script.contains("v.currentTime = p"));
        // 选择器覆盖 bwp-video 自定义元素与多 video 可见过滤。
        assert!(script.contains("bwp-video"));
        assert!(script.contains("videoWidth > 0"));
    }

    #[test]
    fn control_script_next_previous_empty() {
        assert!(control_script(&PlaybackCommandDto::Next).is_empty());
        assert!(control_script(&PlaybackCommandDto::Previous).is_empty());
    }

    #[test]
    fn seek_and_play_control_script_seeks_then_plays() {
        let script = seek_and_play_control_script(12.5, true);
        // play 作为 seekTo 的完成回调（function(){...play(v)...}），确保 seek 到位后再播，
        // 避免 readyState<2 时 play 先以旧位置开播、seek 随后才拉回目标位（「不从 0 开始」根因）。
        assert!(script.contains("__BILI_CTRL__.seekTo(v,12.5,function"));
        assert!(script.contains("__BILI_CTRL__.play(v)"));
        // play 必须在 seekTo 回调函数体内，而非与 seekTo 同级同步调用。
        let call = "__BILI_CTRL__.seekTo(v,12.5,function(){";
        let idx = script.find(call).unwrap();
        let after = &script[idx + call.len()..];
        assert!(after.contains("play(v)"), "play must be inside seekTo onDone callback");
    }

    #[test]
    fn seek_and_play_control_script_skip_play_when_not_requested() {
        let script = seek_and_play_control_script(0.0, false);
        assert!(script.contains("__BILI_CTRL__.seekTo(v,0,function"));
        assert!(!script.contains("__BILI_CTRL__.play(v)"));
    }

    #[test]
    fn play_control_helpers_pick_video_skips_bwp_video_custom_element() {
        // <bwp-video> 是 B 站自定义元素，非 HTMLVideoElement，没有 play()/readyState/videoWidth，
        // 操作它无法控制播放。控制时必须跳过、只接受真正的 <video>。否则早期页面
        // <bwp-video> 先于内部 <video> 出现时会被当作 fallback 返回 → v.play() 抛错/无效
        // → 加载后不自动开播（而用户稍后按播放按钮时 <video> 已就绪，故手动可播）。
        let s = control_script(&PlaybackCommandDto::Play);
        assert!(s.contains("tagName !== 'VIDEO'"), "pickVideo must skip non-<video> nodes");
        assert!(s.contains("bwp-video"), "bwp-video kept in selector for BRIDGE_INIT hook");
        assert!(s.contains("videoWidth > 0"));
    }

    #[test]
    fn play_control_helpers_bootstraps_stream_when_no_metadata() {
        // B 站「自动开播」被我方禁用后，页面加载完 <video> 未必有流（readyState<1）→
        // v.play() 以非 NotAllowedError 失败、静音降级不触发 → 卡住。playV 须在 readyState<1
        // 时点 B 站播放按钮触发拉流（src 在 B 站内部 play() 之前设置，我方守卫拦截其 play()
        // 但 src 已就绪），readyState>=1 时直接 oncePlay；且每轮重新 pickVideo（SPA 可能换元素）。
        let s = control_script(&PlaybackCommandDto::Play);
        assert!(s.contains("readyState < 1"), "click B站 button only when stream not loaded");
        assert!(s.contains("bootstrap"));
        assert!(s.contains("typeof cur.play !== 'function'"), "defensive against non-media nodes");
        assert!(s.contains("pickVideo() || v"), "re-pick video each retry");
    }

    #[test]
    fn play_control_helpers_hook_events_on_found_video() {
        // 关键修复：BRIDGE_INIT 的 document 级 MutationObserver 抓不到 <bwp-video> 内（疑似
        // Shadow DOM）的真 <video> → 事件永不发到 Rust → 前端拿不到 duration（进度条 disabled）
        // 且 ended 不触发切歌。控制脚本找到 <video> 时须自己 hook 事件（emit bili://video-event），
        // 共用 __BILI_HOOKED__ 守卫与 BRIDGE_INIT 互斥。只读 currentTime/duration（合规）。
        let s = control_script(&PlaybackCommandDto::Play);
        assert!(s.contains("ensureHooked"), "control script must hook events on found video");
        assert!(s.contains("bili://video-event"), "must emit video events");
        assert!(s.contains("positionSeconds"), "payload carries position");
        assert!(!s.contains(".src"), "must not read media src");
        assert!(!s.contains("currentSrc"), "must not read currentSrc");
        // poll 与 retry 均须 ensureHooked，覆盖 SPA 切歌换 <video> 元素的场景。
        assert!(s.contains("ensureHooked(v); fn(v)"), "poll hooks before invoking callback");
    }

    #[test]
    fn playback_settings_script_captures_and_disables_requested_settings() {
        let script = playback_settings_script();
        assert!(script.contains("自动切集"));
        assert!(script.contains("自动开播"));
        assert!(script.contains("sessionStorage"));
        assert!(script.contains("captured"));
        assert!(script.contains(".click()"));
    }

    #[test]
    fn bridge_init_blocks_page_autoplay() {
        assert!(BRIDGE_INIT.contains("__BILI_LIST_PLAYER_PLAY_ALLOWANCE__"));
        assert!(BRIDGE_INIT.contains("HTMLMediaElement.prototype.play"));
        assert!(BRIDGE_INIT.contains("autoplay = false"));
        assert!(BRIDGE_INIT.contains("MutationObserver"));
        assert!(BRIDGE_INIT.contains("hookVideo(node)"));
    }

    #[test]
    fn playback_load_waits_for_target_page_before_consuming_pending_commands() {
        assert!(same_list_page(
            "https://www.bilibili.com/video/BV1?from=player",
            "https://www.bilibili.com/video/BV1"
        ));
    }

    #[test]
    fn restore_playback_settings_script_restores_original_values() {
        let script = restore_playback_settings_script();
        assert!(script.contains("__BILI_LIST_PLAYER_RESTORE_SETTINGS__"));
        assert!(script.contains("()"));
    }

    #[test]
    fn playback_commands_accept_frontend_camel_case_positions() {
        let load: PlaybackCommandDto = serde_json::from_value(json!({
            "type": "load",
            "url": "https://www.bilibili.com/video/BV1",
            "positionSeconds": 12.5
        }))
        .unwrap();
        assert_eq!(
            load,
            PlaybackCommandDto::Load {
                url: "https://www.bilibili.com/video/BV1".into(),
                position_seconds: 12.5
            }
        );

        let seek: PlaybackCommandDto = serde_json::from_value(json!({
            "type": "seek",
            "positionSeconds": 4.25
        }))
        .unwrap();
        assert_eq!(
            seek,
            PlaybackCommandDto::Seek {
                position_seconds: 4.25
            }
        );
    }

    #[test]
    fn playback_commands_retain_snake_case_compatibility() {
        let load: PlaybackCommandDto = serde_json::from_value(json!({
            "type": "load",
            "url": "https://www.bilibili.com/video/BV1",
            "position_seconds": 12.5
        }))
        .unwrap();
        assert_eq!(
            load,
            PlaybackCommandDto::Load {
                url: "https://www.bilibili.com/video/BV1".into(),
                position_seconds: 12.5
            }
        );

        let seek: PlaybackCommandDto = serde_json::from_value(json!({
            "type": "seek",
            "position_seconds": 4.25
        }))
        .unwrap();
        assert_eq!(
            seek,
            PlaybackCommandDto::Seek {
                position_seconds: 4.25
            }
        );
    }

    #[test]
    fn validate_good_payload() {
        let v = json!({ "type":"play", "itemId":"/video/BV1x", "positionSeconds":1.2 });
        let p = validate_video_event_payload(&v).unwrap();
        assert_eq!(p.event_type, "play");
        assert_eq!(p.item_id, "/video/BV1x");
        assert_eq!(p.position_seconds, 1.2);
        assert_eq!(p.duration_seconds, None);
    }

    #[test]
    fn validate_payload_with_duration() {
        assert_eq!(
            validate_video_event_payload(&json!({"type":"play","itemId":"/x","positionSeconds":0,"durationSeconds":3.4}))
                .unwrap()
                .duration_seconds,
            Some(3.4)
        );
    }

    #[test]
    fn validate_rejects_unknown_type() {
        assert!(validate_video_event_payload(&json!({"type":"foo","itemId":"/x","positionSeconds":0})).is_none());
    }

    #[test]
    fn validate_rejects_missing_position() {
        assert!(validate_video_event_payload(&json!({"type":"play","itemId":"/x"})).is_none());
    }

    #[test]
    fn validate_rejects_missing_itemid() {
        assert!(validate_video_event_payload(&json!({"type":"play","positionSeconds":0})).is_none());
    }

    #[test]
    fn maps_play_to_started_camelcase() {
        let parsed = ParsedVideoEvent {
            event_type: "play".into(),
            item_id: "/video/BV1x".into(),
            position_seconds: 1.2,
            duration_seconds: None,
        };
        let emit = map_video_event(parsed, "BV1x".into()).unwrap();
        let json = serde_json::to_value(&emit).unwrap();
        assert_eq!(json["type"], "started");
        assert_eq!(json["itemId"], "BV1x");
        assert_eq!(json["positionSeconds"], 1.2);
    }

    #[test]
    fn maps_timeupdate_to_progress_with_duration() {
        let parsed = ParsedVideoEvent {
            event_type: "timeupdate".into(),
            item_id: "/x".into(),
            position_seconds: 5.0,
            duration_seconds: Some(120.0),
        };
        let emit = map_video_event(parsed, "BV2".into()).unwrap();
        let json = serde_json::to_value(&emit).unwrap();
        assert_eq!(json["type"], "progress");
        assert_eq!(json["positionSeconds"], 5.0);
        assert_eq!(json["durationSeconds"], 120.0);
    }

    #[test]
    fn maps_error_to_error_with_message() {
        let parsed = ParsedVideoEvent {
            event_type: "error".into(),
            item_id: "/x".into(),
            position_seconds: 0.0,
            duration_seconds: None,
        };
        let emit = map_video_event(parsed, "BV3".into()).unwrap();
        let json = serde_json::to_value(&emit).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["itemId"], "BV3");
        assert!(json["message"].as_str().unwrap().contains("失败"));
    }

    #[test]
    fn maps_pause_and_ended() {
        let paused = map_video_event(
            ParsedVideoEvent {
                event_type: "pause".into(),
                item_id: "/x".into(),
                position_seconds: 7.0,
                duration_seconds: None,
            },
            "BV4".into(),
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&paused).unwrap()["type"], "paused");
        let ended = map_video_event(
            ParsedVideoEvent {
                event_type: "ended".into(),
                item_id: "/x".into(),
                position_seconds: 99.0,
                duration_seconds: None,
            },
            "BV5".into(),
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&ended).unwrap()["type"], "ended");
    }
}
