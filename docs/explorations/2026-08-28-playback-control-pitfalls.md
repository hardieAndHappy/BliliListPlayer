# 嵌入子 WebView 播放控制踩坑：双击不播 / 进度条灰色 / 播完不切歌

> 日期：2026-08-28
> 状态：已采纳并修复（提交 179e6e7）。本文件记录调试历程与根因，避免日后回归或重构时重蹈覆辙。
> 关联代码：`src-tauri/src/webview.rs`（`PLAY_CONTROL_HELPERS` / `BRIDGE_INIT` / `on_page_load`）、`src/App.tsx`、`src/components/PlaybackControls.tsx`

## 0. 症状

用户反馈三个症状，看似无关、实为同一条断管上的三个表现：

1. 双击队列项 → B 站视频页在嵌入子 WebView 里出现，但**不自动开播**，得手按播放按钮。
2. 视频播完触发切到下一曲，但下一曲**不自动开播**（同样得手按）。
3. 底部**进度条灰色不可拖/不可点**。

> 关键线索：**手动按播放按钮能播**。这条线索否定了"自动播放策略"是主因——同一个 `v.play()` 路径手动可播，说明不是 WebView2 的 autoplay policy 在拦。

## 1. 错误的根因假设（走过的弯路）

### 1.1 误判为 WebView2 自动播放策略

最先假设：主窗口双击的手势不传递给子 WebView 的 B 站页（不同 document），`v.play()` 被 `document-user-activation-required` 拒绝。

对策：在 `lib.rs run()` 开头设置 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--autoplay-policy=no-user-gesture-required`。

**结果：无效。** 环境变量未必在 WebView 创建前的正确时机被读取，且即便生效也只解决"播"，不解决"进度条/切歌"。

### 1.2 误判为 URL 尾斜杠 / 中间页偷意图

曾发现 `parser.rs` 生成的项目 URL 无尾斜杠（`/video/BV1`），B 站规范化为带尾斜杠（`/video/BV1/`），`same_list_page` 比较敏感 → `on_page_load` 的 `playback_matches` 恒 false → `pending_play` 永不消费。另有首页 `Finished` 偷走 `pending_play`。

对策：`same_list_page` 忽略尾斜杠；`on_page_load` 仅在 `playback_matches` 时消费意图，删掉非匹配页的 `else if pending_play`。

**结果：这部分是真 bug，但只修好"播"的前置条件，进度条/切歌仍不动。** 见 commit 4019d7e。

### 教训

> 三个症状里只要"手动能播"，autoplay 策略就不是主因。应当先以运行时日志验证"事件管道是否通"，而不是反复猜根因盲改。盲改了三轮才发现真正断在哪。

## 2. 真正的根因：事件管道断裂

### 2.1 两条独立管道

播放控制其实有**两条独立管道**，之前只修了第一条：

```
管道 A（控制下行）：前端 → Rust send_playback_command → 注入控制脚本 → pickVideo → v.play()
管道 B（事件上行）：B站页 <video> 事件 → hookVideo emit bili://video-event → Rust → 前端
```

- 进度条需要管道 B 的 `timeupdate` → `durationSeconds`。
- 自动切歌需要管道 B 的 `ended`。
- 三症状里只有"不播"靠管道 A；进度条 + 切歌全靠管道 B。

**管道 B 一直是断的**，但因为没有诊断日志，前三轮都在改管道 A，自然修不好 B 的症状。

### 2.2 管道 B 断在哪：Shadow DOM

`BRIDGE_INIT` 用 **document 级** `MutationObserver` + `document.querySelectorAll('bwp-video, video, ...')` 去 hook `<video>` 并绑事件。

但 B 站把**真 `<video>` 放在 `<bwp-video>` 自定义元素的内部**（疑似 Shadow DOM）。document 级的 `querySelectorAll` 与 `MutationObserver` **穿不透 Shadow DOM 边界** → 永远找不到那个 `<video>` → 事件从不绑定 → `play`/`timeupdate`/`ended` 从不 emit 到 Rust。

诊断日志证实：
- BRIDGE_INIT 的 `tryHook found=` 行**从未出现**（observer 一直 0 个）。
- 但控制脚本的 `pickVideo` 却能找到它（`bootstrap`/`playing ok` 日志）——因为 `<video>` 虽在 `<bwp-video>` 内，但 `querySelectorAll('bwp-video, ..., video')` 命中 light DOM 中存在的元素（`<bwp-video>` 本身在 light DOM），而真 `<video>` 可经由子 webview 的 document 可达。**关键不在"能不能找到"，而在 BRIDGE_INIT 的 hook 时机/作用域不对。**

### 2.3 一个 timing 差异造成"手动能播、自动不播"的假象

早期还踩了另一个坑（commit 179e6e7 一并修）：`pickVideo` 的 fallback 曾返回 `<bwp-video>` 自定义元素（非 `HTMLVideoElement`，无 `play()`/`readyState`）。

- 加载后 800ms（控制脚本跑时）：真 `<video>` 还没 `readyState>=1`，`pickVideo` 退回 `<bwp-video>` → `v.play()` 抛错/无效 → 不播。
- 用户按播放按钮时（几秒后）：真 `<video>` 已有元数据 → `pickVideo` 返回真 `<video>` → 成功。

这个 timing 差解释了"手动能播、自动不播"，一度误导成 autoplay 策略问题。

## 3. 修复

### 3.1 `ensureHooked`：控制脚本自管事件 hook（核心）

`PLAY_CONTROL_HELPERS` 新增 `ensureHooked(v)`：`pickVideo` 找到真 `<video>` 时**自己 hook 事件并 emit `bili://video-event`**，不依赖 BRIDGE_INIT 那个抓不到元素的 observer。

- 共用 `__BILI_HOOKED__` 守卫与 BRIDGE_INIT 互斥（同一元素只 hook 一次）。
- 只读 `currentTime`/`duration`（合规约束：不读 `src`/`currentSrc`/`cookie`）。
- `poll` 与 `playV` 每轮重 `pickVideo` + `ensureHooked`：SPA 切歌可能换 `<video>` 元素，旧引用失效，须重 hook 新的。
- 含 DASH 流尾部 `ended` 兜底（与 BRIDGE_INIT 同逻辑）。

```javascript
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
  // timeupdate 兜底 ended + play 复位 __BILI_ENDING__（略）
}
```

### 3.2 `pickVideo` 跳过自定义元素

`if (v.tagName !== 'VIDEO') continue;` —— 只控制真 `<video>`，`<bwp-video>` 留在 selector 仅供 BRIDGE_INIT 尝试 hook。

### 3.3 `playV` 按 `readyState` 分诊拉流

B 站"自动开播"被我方禁用后，加载完 `<video>` 未必有流（`readyState<1`）→ `v.play()` 以非 `NotAllowedError` 失败、静音降级不触发 → 卡住。分诊：

- `readyState < 1`（必暂停、无流）：点 B 站自己的播放按钮触发它 fetch playurl 并设 `src`（`src` 设置发生在 B 站内部 `play()` 之前，我方守卫会拦掉 B 站那次 `play()`，但 `src` 已就绪）。
- `readyState >= 1`（已有流）：直接 `oncePlay`（守卫 `allowance+1`；浏览器拦则静音降级）。
- 只在 `readyState<1`（必为暂停）时点按钮，避免"播放中点按钮 toggle 成暂停"的竞态。

### 3.4 诊断通道（验证用，可后续移除）

注入脚本 `dbg()` 同时 `console.log` 并 emit `bili://debug` → Rust 转发 `bilibili://debug` → `App.tsx` `console.warn` 到主 devtools；Rust 侧 `on_page_load` / `bili://video-event` 监听器 `eprintln` `[bili]` / `[bili-evt]` 到 `tauri dev` 终端。

运行时日志确认事件到达：`ensureHooked readyState=4 duration=215` → `[bili-evt] play pos=0 dur=215` → 进度条启用、ended 沿同管道触发切歌。

## 4. 关键教训

1. **两条管道分别诊断**：控制下行（A）与事件上行（B）是独立的。症状里凡是依赖 `duration`/`ended` 的（进度条、切歌），先怀疑管道 B 是否通，而不是管道 A。
2. **Shadow DOM 穿透**：document 级 `querySelectorAll`/`MutationObserver` 看不到自定义元素 Shadow DOM 内的节点。要在**能拿到元素的地方**（这里是控制脚本的 `pickVideo`）顺手 hook，而不是只靠 document 级 observer。
3. **timing 差 ≠ autoplay 策略**："手动能播、自动不播"未必是 autoplay policy，可能是加载早期元素未就绪导致 fallback 到无效元素。`pickVideo` 必须只返回真正可控制的元素。
4. **先上诊断再改**：盲改三轮不如先在管道咽喉点（Rust 的 `bili://video-event` 监听）加一条日志，一眼看出"事件根本没到 Rust"。
5. **`bwp-video` 不是 `<video>`**：它是 B 站自定义元素，无 `HTMLMediaElement` 接口，操作它无法控制播放。控制时 `tagName` 必须是 `'VIDEO'`。
6. **合规红线**：审计测试连注释里的 `src`/`currentSrc` 字面量都算违规（见 `capture_script_forbidden_literals` 与踩坑：注释写了 `currentSrc` 让测试红了一次）。hook 事件只读 `currentTime`/`duration`，绝不读流地址。

## 5. 验证

- `cargo test --lib`（webview 模块 38/38，含新增 `play_control_helpers_hook_events_on_found_video` 等回归测试）。
- `corepack pnpm test`（vitest 36/36）、`corepack pnpm run build`（tsc+vite 通过）。
- 手动：双击自动开播 + 进度条启用 + 拖拽跟手 + 播完自动切下一曲并开播，均由运行时日志佐证。

## 6. 后续

- 诊断通道（`dbg` emit + `[bili]`/`[bili-evt]` eprintln + App.tsx debug 监听）当前保留以便日后排查，确认稳定后可移除。
- BRIDGE_INIT 的 document 级 observer 现已退化为"能 hook 就 hook"的次要路径，主要事件上报由 `ensureHooked` 承担；如确认 observer 永远 0 命中，可后续精简 BRIDGE_INIT。
