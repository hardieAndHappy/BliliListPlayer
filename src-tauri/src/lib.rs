pub mod model;
pub mod parser;
pub mod storage;
pub mod webview;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use model::{EditEvent, ParsedItem, PlaybackEvent, PlaylistDocument};
use parser::ParseError;
use storage::Storage;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::image::Image;
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

/// 托盘动态菜单项句柄。MenuItem/CheckMenuItem 均 Send+Sync+Clone，存进 Mutex 供事件
/// 监听器按状态更新（播放/暂停标签 + 模式勾选）。None=尚未在 setup 内填充。
#[derive(Default)]
pub struct TrayMenuState {
    /// 「播放/暂停」项，文本随播放态切换（播放中→「暂停」，暂停→「播放」）。
    pub play_pause: Mutex<Option<MenuItem<tauri::Wry>>>,
    /// 4 个播放模式勾选项，key=菜单项 id（"mode-ordered" 等）。
    pub modes: Mutex<Option<HashMap<&'static str, CheckMenuItem<tauri::Wry>>>>,
}

/// 托盘右键菜单动作（Rust emit 到前端，前端复用现有播放 handler）。镜像前端 PlaybackMode。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
struct TrayAction {
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<&'static str>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebView2 默认沿用 Chromium 的 `document-user-activation-required` 自动播放策略：
    // 子 webview 里的 B 站页与主 webview 的 React UI 是不同 document，主窗口双击的手势
    // 不会传递给 B 站页 → 直接 v.play() 会被 NotAllowedError 拒绝，视频不播 → 进度条拿不到
    // duration 而常驻 disabled。放开自动播放策略后，我方显式 allowance+1 的 v.play() 可带声
    // 直接开播；B 站自身自动播放仍由 BRIDGE_INIT 的守卫（allowance=0）拦截，互不干扰。
    #[cfg(target_os = "windows")]
    if std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default().is_empty() {
        std::env::set_var(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "--autoplay-policy=no-user-gesture-required",
        );
    }
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

            // ── 系统托盘 ──
            // 关闭主窗口改为最小化到托盘（后台继续播放）；托盘右键菜单控制播放/模式/退出。
            // 菜单动作 emit 事件到前端，复用其现有播放 handler（goNext/goPrevious/onToggle/applyMode），
            // 不在 Rust 重写播放逻辑。态反映由 Rust 持有菜单项句柄按事件更新（见 webview::register）。
            let show_item = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let prev_item = MenuItem::with_id(app, "prev", "上一首", true, None::<&str>)?;
            // 初始「播放」（未播放）；webview::register 的 video-event 监听器按 play/pause 改文本。
            let play_pause_item = MenuItem::with_id(app, "toggle-play", "播放", true, None::<&str>)?;
            let next_item = MenuItem::with_id(app, "next", "下一首", true, None::<&str>)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            // 4 个播放模式勾选项；初始全不勾，前端启动载入模式后 emit tray-mode 让 Rust 勾上当前项。
            let mode_ordered = CheckMenuItem::with_id(app, "mode-ordered", "顺序播放", true, false, None::<&str>)?;
            let mode_list_loop = CheckMenuItem::with_id(app, "mode-list-loop", "列表循环", true, false, None::<&str>)?;
            let mode_single_loop = CheckMenuItem::with_id(app, "mode-single-loop", "单曲循环", true, false, None::<&str>)?;
            let mode_random = CheckMenuItem::with_id(app, "mode-random", "随机播放", true, false, None::<&str>)?;
            let sep3 = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &show_item, &sep1, &prev_item, &play_pause_item, &next_item, &sep2,
                    &mode_ordered, &mode_list_loop, &mode_single_loop, &mode_random, &sep3,
                    &quit_item,
                ],
            )?;
            let icon = Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
            TrayIconBuilder::with_id("main-tray")
                .icon(icon)
                .menu(&menu)
                .tooltip("BiliListPlayer")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => show_main_window(app),
                    "prev" => emit_tray_action(app, "prev", None),
                    "next" => emit_tray_action(app, "next", None),
                    "toggle-play" => emit_tray_action(app, "toggle-play", None),
                    "quit" => {
                        // 仅在真正退出时还原 B 站播放设置（最小化到托盘时不还原，避免干扰后台播放）。
                        if let Some(wv) = app.get_webview(webview::BILI_LABEL) {
                            let _ = wv.eval(webview::restore_playback_settings_script());
                        }
                        app.exit(0);
                    }
                    id if id.starts_with("mode-") => {
                        // mode-ordered → "ordered" 等，与前端 PlaybackMode kebab-case 对齐。
                        let mode: &'static str = match id {
                            "mode-ordered" => "ordered",
                            "mode-list-loop" => "list-loop",
                            "mode-single-loop" => "single-loop",
                            "mode-random" => "random",
                            _ => return,
                        };
                        emit_tray_action(app, "mode", Some(mode));
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 左键点托盘图标唤起窗口（右键 Windows 自动弹菜单）。
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;
            // 句柄存进 state，供 webview::register 的事件监听器更新标签/勾选。
            let mut modes = HashMap::new();
            modes.insert("mode-ordered", mode_ordered);
            modes.insert("mode-list-loop", mode_list_loop);
            modes.insert("mode-single-loop", mode_single_loop);
            modes.insert("mode-random", mode_random);
            app.manage(TrayMenuState {
                play_pause: Mutex::new(Some(play_pause_item)),
                modes: Mutex::new(Some(modes)),
            });

            // 窗口缩放后通知前端重测播放区 DOM 矩形，校准子 webview 边界。
            // on_window_event 闭包仅收 &WindowEvent 单参；emit_to 按 label 精确投递主 webview。
            let app_handle = app.handle().clone();
            if let Some(main) = app.get_window("main") {
                main.on_window_event(move |event| {
                    match event {
                        tauri::WindowEvent::Resized(_) => {
                            let _ = app_handle.emit_to("main", "bilibili://window-resized", ());
                        }
                        tauri::WindowEvent::CloseRequested { api, .. } => {
                            // 最小化到托盘：阻止关闭、隐藏窗口，应用不退出（隐藏窗口仍存在，
                            // Tauri 仅在窗口 destroy 时退出）。B 站设置仅在「退出」菜单还原。
                            api.prevent_close();
                            if let Some(main) = app_handle.get_window("main") {
                                let _ = main.hide();
                            }
                        }
                        _ => {}
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
            webview::refresh_video_title,
            webview::send_playback_command,
            webview::close_bilibili_webview,
            webview::set_bili_webview_bounds
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}

/// 唤起主窗口：取消最小化 + 显示 + 聚焦，覆盖「最小化」与「隐藏到托盘」两种态。
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(main) = app.get_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
}

/// 托盘菜单动作 → emit 到主 webview，前端 listen 后复用现有播放 handler。
fn emit_tray_action(app: &tauri::AppHandle, action: &'static str, mode: Option<&'static str>) {
    let _ = app.emit_to("main", "bilibili://tray-action", TrayAction { action, mode });
}
