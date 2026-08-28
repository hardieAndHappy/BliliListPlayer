# Chrome Cookie 会话迁移探索

> 日期：2026-08-27  
> 状态：已否决，不进入当前产品实现  
> 目标：用户已在自己的 Chrome 中登录 Bilibili 时，不要求在 BiliListPlayer 的 WebView2 中再次登录。

## 1. 结论

当前产品不采用 Chrome Cookie 会话迁移。专用扩展需要额外安装和发布维护，
Native Messaging 还会增加安装、权限与故障排查成本，不符合当前产品希望降低用户操作负担的目标。

已采用的路线是 **guest-first**：

```text
用户粘贴公开 Bilibili 列表
  → 应用自己的持久化 WebView2 匿名会话直接访问
  → 普通 Bilibili 页面标记为 ready，继续导入或播放
  → 仅当 Bilibili 导航到 Passport 登录/验证页时标记 verification-required
  → 提示用户点击“应用内登录”完成站点要求的验证
```

应用继续使用 `app_data_dir/bili-webview/` 保存自己的 WebView2 会话，
不读取、共享、导入或解密 Chrome Cookie，也不复用 Chrome 默认资料目录。

以下内容保留为被否决方案的技术评估。若未来重新评估，会话迁移可使用
**Chrome 扩展 + Native Messaging + WebView2 Cookie API** 做一次性、显式授权的站点会话迁移：

```text
用户点击 Chrome 扩展中的“连接 BiliListPlayer”
  → 扩展通过 chrome.cookies 读取 *.bilibili.com Cookie
  → Native Messaging 把限定字段交给本机 BiliListPlayer
  → Rust 校验来源、域名、Cookie 属性和数量
  → Tauri Webview::set_cookie 写入 bili-webview 独立数据目录
  → WebView2 打开 Bilibili 页面验证会话
  → 迁移消息立即从内存清除，不写日志和业务数据文件
```

这里的“不需要登录”是 **复用用户已经主动登录的 Chrome 会话，免去第二次登录**，不是绕过 Bilibili 的登录、风控、权限或账号验证。

## 2. 为什么不能直接共享 Chrome 资料目录

- 项目当前使用的是 Windows WebView2。它虽然基于 Chromium，但拥有独立的 User Data Folder，当前路径为 `app_data_dir/bili-webview/`。
- Chromium 内核相同不代表 Chrome 与 WebView2 的 Cookie 存储、加密密钥、进程锁和资料目录生命周期兼容。
- WebView2 只允许配置兼容的 WebView2 环境共享同一个 User Data Folder；Chrome 默认 Profile 不是受支持的 WebView2 User Data Folder。把它直接传给 `WebviewBuilder::data_directory` 不能作为可靠的会话共享方式。
- 从 Chrome 136 开始，远程调试参数不能再用于默认 Chrome 数据目录；必须指定非默认 `--user-data-dir`，而新目录使用不同的加密密钥。因此 CDP 连接默认资料目录并提取 Cookie 不是稳定方案。

## 3. 为什么不读取 Cookie 数据库

不采用以下实现：

- 直接读取 Chrome `Network/Cookies` SQLite 数据库。
- 复制正在使用的 Chrome Profile 后离线解密。
- 使用 DPAPI/App-Bound Encryption 绕过 Chrome 的 Cookie 保护。
- 启动带远程调试端口的默认 Chrome Profile 抓取 Cookie。
- 将完整 Cookie 明文写入 JSON、日志、剪贴板或播放列表数据。

这些方案与 Chrome 当前的安全模型冲突，版本兼容性差，也会把应用变成浏览器凭据提取器。

## 4. 推荐组件

### 4.1 Chrome 扩展

Manifest V3 最小权限：

```json
{
  "permissions": ["cookies", "nativeMessaging"],
  "host_permissions": [
    "https://bilibili.com/*",
    "https://*.bilibili.com/*"
  ]
}
```

扩展只响应用户主动点击，不在后台定时读取 Cookie。读取时使用 `chrome.cookies.getAll({ domain: "bilibili.com" })`，不请求其他站点权限。

### 4.2 Native Messaging Host

- Host 名称固定，例如 `com.bililistplayer.session_bridge`。
- `allowed_origins` 只允许发布版扩展 ID，不使用通配符。
- Windows 下由应用安装/首次配置流程写入当前用户的 Native Messaging Host 注册项。
- 消息使用一次性 `requestId`，限制消息大小、Cookie 数量、名称长度和值长度。
- Host 仅把消息交给当前用户会话中的 BiliListPlayer，不提供任意命令执行、文件读取或 URL 导航能力。

### 4.3 Tauri / WebView2

Tauri 2.11.5 已提供以下后端 API：

- `Webview::set_cookie`
- `Webview::cookies_for_url`
- `Webview::delete_cookie`

导入逻辑应放在 Rust 后端，并且只操作 label 为 `bilibili` 的子 WebView。远程 Bilibili 页面仍然不能直接调用 Cookie 导入命令；导入只能由本地主窗口发起。

## 5. 数据契约

扩展到 Native Host 的 Cookie DTO 只包含：

```text
name
value
domain
path
secure
httpOnly
sameSite
expirationDate（可选）
hostOnly
session
```

Rust 必须拒绝：

- 非 `bilibili.com` 或非 `.bilibili.com` 域。
- 空名称、控制字符、超限字段。
- 非 HTTPS 站点所需的 Secure Cookie 降级。
- 未知 SameSite 值。
- 扩展来源 ID、请求 nonce 或协议版本不匹配。

实现不应硬编码或只挑选 `SESSDATA` 等特定 Cookie 名称；按域和安全属性迁移，避免跟随站点内部 Cookie 命名变化。

## 6. 被否决方案的原用户体验

原方案建议把现有“打开登录页”调整为两个入口：

1. `从 Chrome 连接`：检测扩展和 Native Host，用户确认后迁移 Bilibili 会话。
2. `在应用内登录`：保留当前 WebView2 正常登录作为无 Chrome、扩展不可用、会话失效或风控验证时的回退。

导入后只显示结果：

- 已连接 Chrome 会话。
- Chrome 中没有可用的 Bilibili 登录会话。
- 扩展未安装或 Native Host 未注册。
- 会话已过期，请在 Chrome 或应用内重新验证。

界面、日志和错误信息不得显示 Cookie 名称和值。

## 7. 对现有需求的影响

现有需求文档和审计明确禁止 Chrome Cookie 读取/导入。正式实现前必须先批准并同步修改：

- `docs/superpowers/specs/2026-08-26-bili-list-player-design.md`
- `docs/audits/2026-08-26-audit.md`
- `tests/audit/no-cookie-import.test.ts`
- `docs/index.md` 的安全边界

新的安全边界应是：

- 禁止读取或解密浏览器 Cookie 数据库。
- 允许用户安装的扩展通过 Chrome 官方 `chrome.cookies` API，只迁移 Bilibili 域 Cookie。
- Cookie 只进入 WebView2 Cookie Store，不进入应用业务存储、历史、日志或崩溃报告。
- 随时支持清除 BiliListPlayer 自己的 Bilibili 会话。

## 8. 验证清单

- Chrome 已登录、WebView2 未登录：迁移后可识别登录会话。
- Chrome 未登录：不制造伪登录状态，提示无可用会话。
- 多 Chrome Profile：只使用用户触发扩展所在 Profile 的 Cookie Store。
- Incognito：默认不支持；除非用户显式允许扩展在无痕模式运行。
- Cookie 过期、SameSite、HttpOnly、Secure、Session 属性迁移正确。
- Native Host 来源伪造、重复 requestId、超大消息和非 Bilibili 域均被拒绝。
- 退出应用后 WebView2 会话仍由 `bili-webview` 数据目录持久化。
- 清除应用会话不会删除或修改 Chrome 中的 Cookie。

## 9. 参考

- Chrome Cookies API: <https://developer.chrome.com/docs/extensions/reference/api/cookies>
- Chrome Native Messaging: <https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging>
- Chrome 136 远程调试变更: <https://developer.chrome.com/blog/remote-debugging-port>
- WebView2 User Data Folder: <https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/user-data-folder>
- WebView2 Cookie Manager: <https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2cookiemanager>
