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

/// 比较两个 URL 是否指向同一列表页（host+path），忽略 query/fragment。
/// B 站常规范化 query（如去掉 oid/bvid），精确 `src == url` 会导致 capture 永不触发（20s 超时）。
fn same_list_page(a: &str, b: &str) -> bool {
    match (Url::parse(a).ok(), Url::parse(b).ok()) {
        (Some(x), Some(y)) => x.host_str() == y.host_str() && x.path() == y.path(),
        _ => a == b,
    }
}

const BRIDGE_INIT: &str = r#"(function(){
  if (window.__BILI_BRIDGE__) return; window.__BILI_BRIDGE__ = true;
  function allowedHost(h){ return h==='www.bilibili.com'||h==='bilibili.com'||h==='passport.bilibili.com'||h.endsWith('.bilibili.com'); }
  function emit(event, payload){
    try { window.__TAURI_INTERNALS__.invoke('plugin:event|emit', { event: event, payload: payload }); } catch(e){}
  }
  if (window.location.protocol !== 'https:' || !allowedHost(window.location.hostname)) return;
  emit('bili://page-loaded', { url: window.location.href });
  var SELECTORS = 'video, .bpx-player-container video, .bilibili-player video';
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
  }
  function tryHook(){
    var nodes = document.querySelectorAll(SELECTORS);
    for (var i=0;i<nodes.length;i++) hookVideo(nodes[i]);
    if (nodes.length === 0 && Date.now() < deadline) setTimeout(tryHook, 500);
  }
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

fn control_script(cmd: &PlaybackCommandDto) -> String {
    let selectors = "video, .bpx-player-container video, .bilibili-player video";
    match cmd {
        PlaybackCommandDto::Play => format!(
            r#"(function(){{var v=document.querySelector('{s}');if(v){{v.play().catch(function(){{}});}}}})();"#,
            s = selectors
        ),
        PlaybackCommandDto::Pause => format!(
            r#"(function(){{var v=document.querySelector('{s}');if(v){{v.pause();}}}})();"#,
            s = selectors
        ),
        PlaybackCommandDto::Seek { position_seconds } => format!(
            r#"(function(){{var v=document.querySelector('{s}');if(v){{v.currentTime={p};}}}})();"#,
            s = selectors,
            p = position_seconds
        ),
        PlaybackCommandDto::Next | PlaybackCommandDto::Previous | PlaybackCommandDto::Load { .. } => {
            String::new()
        }
    }
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
    login_state: String, // "guest" | "unknown"
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

    let app_p = app.clone();
    app.listen("bili://page-loaded", move |event| {
        let value: serde_json::Value = match serde_json::from_str(event.payload()) {
            Ok(v) => v,
            Err(_) => return,
        };
        let Some(url) = value.get("url").and_then(|v| v.as_str()) else {
            return;
        };
        let login_state = match Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
        {
            Some(h) if h == "passport.bilibili.com" => "guest".to_string(),
            _ => "unknown".to_string(),
        };
        let _ = app_p.emit_to(
            "main",
            "bilibili://page-state",
            PageStatePayload {
                url: url.to_string(),
                login_state,
            },
        );
    });

    Ok(())
}

/// 获取或创建 Bilibili 子 webview（嵌入主窗口内，非弹窗）。
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
            let pending_seek = state.pending_seek.lock().unwrap().take();
            if let Some(pos) = pending_seek {
                let wv = webview.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(800));
                    let _ = wv.eval(&control_script(&PlaybackCommandDto::Seek {
                        position_seconds: pos,
                    }));
                });
            }
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
        PlaybackCommandDto::Play | PlaybackCommandDto::Pause | PlaybackCommandDto::Seek { .. } => {
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
        assert!(control_script(&PlaybackCommandDto::Play).contains("v.play()"));
    }

    #[test]
    fn control_script_pause() {
        assert!(control_script(&PlaybackCommandDto::Pause).contains("v.pause()"));
    }

    #[test]
    fn control_script_seek() {
        assert!(control_script(&PlaybackCommandDto::Seek {
            position_seconds: 1.5
        })
        .contains("v.currentTime=1.5"));
    }

    #[test]
    fn control_script_next_previous_empty() {
        assert!(control_script(&PlaybackCommandDto::Next).is_empty());
        assert!(control_script(&PlaybackCommandDto::Previous).is_empty());
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
