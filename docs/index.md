# BiliListPlayer — 构件索引

> 本文件是项目的**语义锚点**：按概念定位到具体代码位置。改某个功能时，先来这里找入口。
> 维护原则：改了架构/接线/契约就同步更新本表。行号会漂移，以函数名为准、行号为辅。

## 1. 这是什么

Windows 桌面应用，本地管理 Bilibili 视频列表：**窗口内嵌入 WebView 在线播放** + **本地列表编辑/排序/循环/随机**。明确**不下载媒体**——只读 `currentTime`/`duration`/`outerHTML`，从不读 `src`/`currentSrc`/`cookie`。

| 层 | 技术 | 版本 |
|---|---|---|
| 外壳 | Tauri 2（`unstable` feature，用 `window.add_child` 嵌子 webview） | 2.11.5 |
| 后端 | Rust（无外部运行时，纯 std + serde + url） | edition 2021 |
| 前端 | React + TypeScript + Vite | React 18 / Vite 5 |
| 测试 | Rust `cargo test --lib`（43 单测）· 前端 `vitest` |

## 2. 目录结构（源码）

```
src-tauri/
├─ Cargo.toml                      # tauri features=["unstable"]（add_child 必需）
├─ tauri.conf.json                 # 主窗口 label="main"；CSP 白名单 bilibili 域
├─ capabilities/
│  ├─ default.json                 # webviews:["main"] → core:default
│  └─ bilibili-remote.json         # webviews:["bilibili"] 远程页只给 event emit/listen
└─ src/
   ├─ main.rs                      # 入口，仅调 lib::run()
   ├─ lib.rs                       # 命令注册 + setup + 窗口缩放监听
   ├─ model.rs                     # 全部 DTO（Rust 端真相）
   ├─ parser.rs                    # 列表 HTML 解析适配器（纯函数）
   ├─ storage.rs                   # 本地 JSON / JSONL 落盘
   └─ webview.rs                   # Bilibili 子 webview 生命周期 + 注入脚本 + 事件路由

src/
├─ main.tsx                        # React 挂载点
├─ App.tsx                         # 唯一有状态组件（状态机中枢）
├─ styles.css                      # 单文件样式 + grid 布局
├─ types/playlist.ts               # 前端 DTO（镜像 Rust model.rs）
├─ components/
│  ├─ PlaylistQueue.tsx            # 队列列表（精简行：名字 + 小×）
│  └─ PlaybackControls.tsx         # 底栏图标控制（⏮ ▶/⏸ ⏭ + 进度 + 循环）
└─ services/                       # 纯逻辑 / IPC 适配，无 React
   ├─ webviewStore.ts              # 子 webview 命令封装（bounds/navigate/login）
   ├─ playbackBridge.ts            # 播放命令/事件 IPC 桥
   ├─ parseService.ts             # 抓取+解析协调（capture_list_html + 监听结果）
   ├─ playlistStore.ts             # 列表文档 load/save IPC
   ├─ historyStore.ts              # 编辑/播放历史追加 IPC
   ├─ playbackReducer.ts           # 播放事件 → UI 状态（纯函数）
   ├─ playbackProgress.ts          # 播放条目进度更新（纯函数）
   ├─ playbackMode.ts              # 下一首/上一首算法（4 模式，纯函数）
   ├─ bilibiliPageState.ts         # ready/verification-required 前端策略
   ├─ bilibiliParser.ts            # 前端 URL 预检（权威校验在 Rust）
   └─ importSelection.ts           # （历史辅助，现导入流不再走预览）
```

### Bilibili 会话路线

当前采用 **guest-first**：公开列表和视频直接使用应用自己的持久化 WebView2
匿名会话，不要求用户先登录。页面加载完成后，Rust 发出两种访问状态：

- `ready`：公开页面可继续抓取或播放。
- `verification-required`：Bilibili 已导航到 Passport 登录/验证页，此时才提示“应用内登录”。

WebView2 会话仍持久化在 `app_data_dir/bili-webview/`。不读取、共享、导入或解密
Chrome Cookie，也不要求安装浏览器扩展。此前的
[Chrome Cookie 会话迁移探索](explorations/2026-08-27-chrome-cookie-handoff.md)
已被当前产品路线否决，仅保留为历史决策记录。

## 3. 架构分层与数据流

四层，单向依赖（UI → 服务 → IPC → Rust Core）。**Rust 是权威**，前端 DTO 镜像 Rust `model.rs`。

```
┌─────────────── React UI（App.tsx 单组件持全部状态）───────────────┐
│  playlists/active/currentItemId/mode/playback/queueWidth/ctxMenu   │
└──────────┬──────────────────────────────────────┬─────────────────┘
           │ invoke 命令                          │ listen 事件
┌──────────▼──────── 服务层（services/*.ts，无状态）────────────────▼─────┐
│ webviewStore · playbackBridge · parseService · playlistStore · history  │
│ playbackReducer · playbackProgress · playbackMode · bilibiliParser  ← 纯函数，可单测 │
└──────────┬──────────────────────────────────────────────────────────────┘
           │ Tauri IPC（invoke / event）
┌──────────▼──────── Rust Core（src-tauri/src/）──────────────────────────┐
│ lib.rs   命令注册 + setup（manage Storage/CaptureState + 窗口缩放 emit） │
│ webview.rs  子 webview 生命周期 + 注入脚本 + bilibili:// 事件路由         │
│ parser.rs   列表 HTML → ParsedItem[]（纯函数）                          │
│ storage.rs  PlaylistDocument + 历史 JSONL 落盘                           │
│ model.rs    全部 DTO（serde camelCase，与前端对齐）                       │
└─────────────────────────────────────────────────────────────────────────┘
```

**导入数据流**（典型）：
`App.importList` → 前端预检 → `navigateBilibili` → `captureAndParse`(invoke `capture_list_html`) → Rust 注入 `capture_script` 到 B 站页 → 脚本抓 `__INITIAL_STATE__` + 分页 API → emit `bili://capture-html` → Rust `register` 路由 → `parser::parse_list_html` → emit `bilibili://parse-result` → 前端落库 → `save_playlists`。

**播放数据流**：
`App.handlePlay` → `sendCommand({load})`+`{play}` → invoke `send_playback_command`（load=navigate+pending_seek+pending_playback_url，play 置 pending_play）→ 目标页 `Finished` 且 `same_list_page` 匹配（忽略 query/尾斜杠）后 eval `seek_and_play_control_script`（轮询等 `<video>`→seek→play，带静音降级）→ B 站页 `<video>` 事件 → emit `bili://video-event` → Rust `map_video_event` 规范化 → emit `bilibili://playback-event` → `playbackReducer` → UI。上一曲/下一曲及自动切歌固定使用开始播放时的列表上下文；切换侧边栏浏览其他列表不会改变实际播放列表的导航范围。

## 4. UI 结构

单窗口 grid 布局（[styles.css](../src/styles.css) `.app-shell`），无标题栏。列表区和队列区宽度分别由 `--sidebar-w`、`--queue-w` CSS 变量驱动（localStorage 持久化），两条分割线均可拖动；播放器跨满整个窗口高度。

```
.app-shell  grid-template: "sidebar sidebar-splitter queue splitter player" 1fr / "controls controls controls controls player" 70px
列: var(--sidebar-w) | 8px | var(--queue-w) | 8px | 1fr

┌──────────┬──────────────┬──┬─────────────────────┐
│ 列表区    │ 队列区        │栏│  WebView 播放区       │  ← player 跨两行，满高最大化
│ sidebar  │ queue        │  │  (原生子 webview 覆盖) │
│          │              │  │  .webview-placeholder  │
│          │              │  │   应用内登录 / 显示WebView│
├──────────┴──────────────┴──┤                       │
│   播放控制区 controls         │                       │  ← 在列表+队列下方，不压播放器
│   ⏮ ▶ ⏭ ━━━━━━ 0:00/0:00 🔁  │                       │
└─────────────────────────────┴───────────────────────┘
```

- **列表区**：`.sidebar-scroll`(「＋ 新增列表」虚线按钮 → 各列表项 → 最近播放/编辑历史)。列表区和队列区之间的 `.sidebar-splitter` 可拖动调整宽度。列表项 `onContextMenu` 弹 `.ctx-menu`(重命名/删除)。
- **队列区**：标题 + 来源 URL + notice + `.playlist-row`(双击播放、×删除)。
- **播放区**：`playerRef` 测矩形上报 Rust 校准子 webview，播放器跨满窗口上下高度。占位 HTML 仅在 webview 隐藏时可见。
- **z-order 关键**：原生子 webview 永远在 HTML 之上 → 导入/编辑时 `webviewVisibleRef=false` 上报 `(0,0,0,0)` 隐藏；左侧切换本地列表不导航、不刷新、不隐藏当前 WebView，只有队列区选择视频或播放区操作才改变 WebView。

## 5. 关键代码位置（按概念锚定）

### Rust 后端

| 概念 | 位置 |
|---|---|
| 命令注册 + setup + 窗口缩放 emit | [lib.rs:66](../src-tauri/src/lib.rs#L66) `run()` |
| WebView2 放开自动播放策略（`--autoplay-policy=no-user-gesture-required`） | [lib.rs:69-78](../src-tauri/src/lib.rs#L69) |
| `Storage` / `CaptureState` 注入 | [lib.rs:86-88](../src-tauri/src/lib.rs#L86) |
| 窗口 Resized → `bilibili://window-resized` | [lib.rs:95-103](../src-tauri/src/lib.rs#L95) |
| 子 webview label / home 常量 | [webview.rs:17-18](../src-tauri/src/webview.rs#L17) |
| 主机白名单 `is_allowed_bili_host` | [webview.rs:21](../src-tauri/src/webview.rs#L21) |
| `same_list_page`（host+path 忽略 query 与尾斜杠） | [webview.rs:42](../src-tauri/src/webview.rs#L42) |
| `BRIDGE_INIT` 注入脚本（hook `<video>` 事件） | [webview.rs:58](../src-tauri/src/webview.rs#L58) |
| `capture_script`（抓 `__INITIAL_STATE__`+分页+`listTitle`） | [webview.rs:132](../src-tauri/src/webview.rs#L132) |
| `PLAY_CONTROL_HELPERS`（轮询 `<video>` + 静音降级播放） | [webview.rs:343](../src-tauri/src/webview.rs#L343) |
| `control_script`（play/pause/seek，轮询等 `<video>`） | [webview.rs:365](../src-tauri/src/webview.rs#L365) |
| `seek_and_play_control_script`（Load 流程合并 seek+play） | [webview.rs:384](../src-tauri/src/webview.rs#L384) |
| 视频事件校验 `validate_video_event_payload` | [webview.rs:408](../src-tauri/src/webview.rs#L408) |
| 视频事件规范化 `map_video_event` | [webview.rs:470](../src-tauri/src/webview.rs#L470) |
| 事件路由 `register`（3 个 bili:// 监听） | [webview.rs:516](../src-tauri/src/webview.rs#L516) |
| 子 webview 创建 `ensure_bili_webview` | [webview.rs:605](../src-tauri/src/webview.rs#L605) |
| 命令 `open_bilibili_webview` | [webview.rs:704](../src-tauri/src/webview.rs#L704) |
| 命令 `navigate_bilibili_webview`（**不 show**） | [webview.rs:718](../src-tauri/src/webview.rs#L718) |
| 命令 `capture_list_html` | [webview.rs:739](../src-tauri/src/webview.rs#L739) |
| 命令 `send_playback_command` | [webview.rs:772](../src-tauri/src/webview.rs#L772) |
| 命令 `close_bilibili_webview`（hide 不销毁） | [webview.rs:840](../src-tauri/src/webview.rs#L840) |
| 命令 `set_bili_webview_bounds`（show/hide 真相源） | [webview.rs:851](../src-tauri/src/webview.rs#L851) |
| `CaptureState`（current_url/pending_capture/pending_seek/pending_play/last_bounds） | [webview.rs:426](../src-tauri/src/webview.rs#L426) |
| URL 校验/规范化 `validate_list_url` | [parser.rs:36](../src-tauri/src/parser.rs#L36) |
| 视频 id 规范化 `normalize_video_id` | [parser.rs:62](../src-tauri/src/parser.rs#L62) |
| 列表 HTML 解析 `parse_list_html` | [parser.rs:90](../src-tauri/src/parser.rs#L90) |
| 列表文档落盘 `save_playlist`（tmp+rename+backup） | [storage.rs:58](../src-tauri/src/storage.rs#L58) |
| 历史 JSONL 追加 + 轮转（上限 10000） | [storage.rs:82](../src-tauri/src/storage.rs#L82) |
| 全部 DTO 定义 | [model.rs](../src-tauri/src/model.rs) |

### 前端

| 概念 | 位置 |
|---|---|
| 根组件（唯一有状态） | [App.tsx:41](../src/App.tsx#L41) `App()` |
| 状态声明 | [App.tsx:42-59](../src/App.tsx#L42) |
| 播放区矩形上报 `measureAndReportBounds` | [App.tsx:65](../src/App.tsx#L65) |
| 加载/保存文档 effect | [App.tsx:80-92](../src/App.tsx#L80) |
| 播放事件分发 `handleEvent` | [App.tsx:136](../src/App.tsx#L136) |
| 导入主流程 `importList` | [App.tsx:209](../src/App.tsx#L209) |
| 新增列表 `handleAddList`（prompt→importList） | [App.tsx:243](../src/App.tsx#L243) |
| 落库 `applyImport`（去重追加/新建，用 listTitle） | [App.tsx:249](../src/App.tsx#L249) |
| 删除整列表 `handleDeletePlaylist` | [App.tsx:306](../src/App.tsx#L306) |
| 重命名 `handleRenamePlaylist` | [App.tsx:329](../src/App.tsx#L329) |
| 分割栏拖拽 `onSplitterMouseDown` | [App.tsx:348](../src/App.tsx#L348) |
| 播放 `handlePlay` | [App.tsx:368](../src/App.tsx#L368) |
| 打开登录/显示 webview | [App.tsx:379-380](../src/App.tsx#L379) |
| JSX 布局 | [App.tsx:382](../src/App.tsx#L382) |
| 子 webview 命令封装 | [webviewStore.ts](../src/services/webviewStore.ts) |
| `setBilibiliBounds`（0,0,0,0=隐藏） | [webviewStore.ts:12](../src/services/webviewStore.ts#L12) |
| 播放命令/事件类型 + IPC 桥 | [playbackBridge.ts](../src/services/playbackBridge.ts) |
| 抓取协调 + 风控早退 + 超时 | [parseService.ts:13](../src/services/parseService.ts#L13) `captureAndParse` |
| 列表 store IPC | [playlistStore.ts:17](../src/services/playlistStore.ts#L17) `createTauriStore` |
| 播放事件→UI 状态 reducer | [playbackReducer.ts:16](../src/services/playbackReducer.ts#L16) |
| 播放条目进度更新 | [playbackProgress.ts](../src/services/playbackProgress.ts) `updatePlaylistItemPosition` |
| 下一首/上一首 4 模式算法 | [playbackMode.ts:3](../src/services/playbackMode.ts#L3) |
| 前端 URL 预检 | [bilibiliParser.ts:7](../src/services/bilibiliParser.ts#L7) `validateListUrl` |
| 前端 DTO（镜像 Rust） | [types/playlist.ts](../src/types/playlist.ts) |
| 队列组件 | [PlaylistQueue.tsx](../src/components/PlaylistQueue.tsx) |
| 控制栏组件（图标） | [PlaybackControls.tsx](../src/components/PlaybackControls.tsx) |
| grid 布局 + 各区域样式 | [styles.css](../src/styles.css) `.app-shell` |

## 6. IPC 契约（命令 / 事件）

**前端 → Rust（`invoke` 命令，注册于 [lib.rs:85](../src-tauri/src/lib.rs#L85)）**

| 命令 | 入参 | 作用 |
|---|---|---|
| `load_playlists` / `save_playlists` | `{document?}` | 列表文档读写 |
| `parse_list_html` | `{sourceUrl, html}` | 纯解析（无网络） |
| `append_edit_event` / `append_playback_event` | `{event}` | 历史追加（eventId Rust 填） |
| `open_bilibili_webview` | `{url?}` | 创建/显示子 webview，可选 navigate |
| `navigate_bilibili_webview` | `{url}` | 导航（**不 show**） |
| `capture_list_html` | `{sourceUrl, requestId}` | 抓列表页 HTML |
| `send_playback_command` | `{command}` | load/play/pause/seek/next/prev |
| `close_bilibili_webview` | — | hide（不销毁，保登录态） |
| `set_bili_webview_bounds` | `{x,y,width,height}` | 校准位置/尺寸；0=隐藏（**可见性真相源**） |

**B 站页 → Rust → 前端（事件，路由于 [webview.rs:308](../src-tauri/src/webview.rs#L308) `register`）**

| 注入脚本 emit | Rust 转发到 main | 前端监听于 |
|---|---|---|
| `bili://video-event` | `bilibili://playback-event` | [playbackBridge.ts:33](../src/services/playbackBridge.ts#L33) |
| `bili://capture-html` | `bilibili://parse-result` | [parseService.ts:19](../src/services/parseService.ts#L19) |
| `bili://page-loaded` | `bilibili://page-state` | [App.tsx](../src/App.tsx)（`ready` 继续；`verification-required` 提示验证） |

**Rust → 前端（窗口事件）**：`bilibili://window-resized`（[lib.rs:79](../src-tauri/src/lib.rs#L79)）→ [App.tsx:203](../src/App.tsx#L203) 重测 bounds。

## 7. 数据模型

权威定义在 [model.rs](../src-tauri/src/model.rs)，前端镜像于 [types/playlist.ts](../src/types/playlist.ts)。serde 全 `camelCase`（与前端一致），枚举 `kebab-case`。

- `PlaylistDocument` → `LocalPlaylist[]` → `PlaylistItem[]`（id=BV/av 规范化、url、status、position、lastPositionSeconds）
- `LocalPlaylist.playback`: `PlaybackContext`（mode、currentItemId、randomSeed、randomRound）
- `PlaybackMode`: `ordered` | `list-loop` | `single-loop` | `random`
- `ParsedItem`: 解析适配器输出（无 position/playCount 等本地字段）
- `EditEvent` / `PlaybackEvent`: 历史记录（eventId 由 Rust `generate_event_id` 填充，[lib.rs:17](../src-tauri/src/lib.rs#L17)）

落盘位置（`app_data_dir/data/`）：`playlists.json`(+`.backup`+`.corrupt.<ts>`) · `edit-history.jsonl` · `playback-history.jsonl`（各上限 10000 行）。子 webview cookie：`app_data_dir/bili-webview/`（登一次重启免登）。

## 8. 安全约束（不可破坏）

- **远程 B 站子 webview 只授 `core:event:allow-emit` + `allow-listen`**，绝不给应用自定义命令——防 [CVE-2024-35222](https://nvd.nist.gov/vuln/detail/CVE-2024-35222)。见 [bilibili-remote.json](../src-tauri/capabilities/bilibili-remote.json)（`webviews:["bilibili"]` 非 `windows`）。
- **注入脚本只读 `currentTime`/`duration`/`outerHTML`**，禁止读 `src`/`currentSrc`/`cookie`。审计测试 grep 这些字面量（见 [webview.rs:677](../src-tauri/src/webview.rs#L677) `capture_script_forbidden_literals`）。
- **只提取 `/video/<BV|av>` 页面链接**，排除 playurl/流地址/签名参数——不执行下载（[parser.rs:85-90](../src-tauri/src/parser.rs#L85)）。
- 主窗口 CSP 白名单 bilibili/hdslb/bilivideo 域（[tauri.conf.json:23](../src-tauri/tauri.conf.json#L23)）。

## 9. 开发与构建

```bash
corepack pnpm tauri dev      # 开发（Vite + cargo run，热更前端）
corepack pnpm run build      # 前端 tsc + vite build（类型校验）
cd src-tauri && cargo test --lib   # Rust 单测（43 项，含纯函数 + 注入脚本字面量审计）
corepack pnpm test           # 前端 vitest（reducer/mode/parser/bridge/importSelection）
```

临时产物/测试文件不入库（`tests/`、`*.test.ts` 已 gitignore）。
