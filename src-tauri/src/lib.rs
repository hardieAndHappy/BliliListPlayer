pub mod model;
pub mod parser;
pub mod storage;
pub mod webview;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use model::{EditEvent, ParsedItem, PlaybackEvent, PlaylistDocument};
use parser::ParseError;
use storage::Storage;
use tauri::{Emitter, Manager, State};

static EVENT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成进程内唯一的事件 id：纳秒时间戳 + 自增计数器 + 事件类型。无需 uuid 依赖。
fn generate_event_id(event_type: &str) -> String {
    let n = EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{:x}-{}", nanos, n, event_type)
}

#[tauri::command]
fn load_playlists(storage: State<'_, Storage>) -> Result<Option<PlaylistDocument>, String> {
    storage.load_playlist().map_err(|error| error.to_string())
}

#[tauri::command]
fn save_playlists(storage: State<'_, Storage>, document: PlaylistDocument) -> Result<(), String> {
    storage.save_playlist(&document).map_err(|error| error.to_string())
}

/// 解析列表页 HTML 为结构化项目 DTO（§7：解析适配器位于 Rust Core，UI 解耦）。
/// 本命令不做网络抓取；HTML 由前端（第 ③ 步 WebView 桥接）喂入。
#[tauri::command]
fn parse_list_html(source_url: String, html: String) -> Result<Vec<ParsedItem>, String> {
    parser::validate_list_url(&source_url).map_err(|e: ParseError| e.to_string())?;
    Ok(parser::parse_list_html(&source_url, &html))
}

/// 追加编辑历史事件（§5.4）。前端不传 eventId，由本命令服务端填充。
#[tauri::command]
fn append_edit_event(storage: State<'_, Storage>, mut event: EditEvent) -> Result<(), String> {
    if event.event_id.is_none() {
        event.event_id = Some(generate_event_id(&event.event_type));
    }
    storage.append_edit_event(&event).map_err(|e| e.to_string())
}

/// 追加播放历史事件（§5.4）。前端不传 eventId，由本命令服务端填充。
#[tauri::command]
fn append_playback_event(
    storage: State<'_, Storage>,
    mut event: PlaybackEvent,
) -> Result<(), String> {
    if event.event_id.is_none() {
        event.event_id = Some(generate_event_id(&event.event_type));
    }
    storage.append_playback_event(&event).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 便携版：数据存 exe 同级目录（免安装、随 exe 走）。
            // 用 std::env::current_exe 取 exe 路径绕过 Tauri 路径 API 的 unknown path
            //（app.path().executable_dir() 在某些环境返回 unknown path 导致 panic）。
            let exe = std::env::current_exe()?;
            let data_dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            app.manage(Storage::new(&data_dir)?);
            webview::register(app.handle()).map_err(|e| e.to_string())?;
            app.manage(webview::CaptureState::default());
            // 窗口缩放后通知前端重测播放区 DOM 矩形，校准子 webview 边界。
            // on_window_event 闭包仅收 &WindowEvent 单参；emit_to 按 label 精确投递主 webview。
            let app_handle = app.handle().clone();
            if let Some(main) = app.get_window("main") {
                main.on_window_event(move |event| {
                    if let tauri::WindowEvent::Resized(_) = event {
                        let _ = app_handle.emit_to("main", "bilibili://window-resized", ());
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_playlists,
            save_playlists,
            parse_list_html,
            append_edit_event,
            append_playback_event,
            webview::open_bilibili_webview,
            webview::navigate_bilibili_webview,
            webview::capture_list_html,
            webview::send_playback_command,
            webview::close_bilibili_webview,
            webview::set_bili_webview_bounds
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
