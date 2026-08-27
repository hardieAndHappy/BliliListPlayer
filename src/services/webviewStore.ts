import { invoke } from '@tauri-apps/api/core';

export const BILIBILI_HOME = 'https://www.bilibili.com';
export const BILIBILI_LOGIN_URL = `${BILIBILI_HOME}/account/login`;
export const BILIBILI_LIST_HINT = `${BILIBILI_HOME}/list/...`;

export const openBilibiliLogin = () => invoke('open_bilibili_webview', { url: BILIBILI_LOGIN_URL });
export const navigateBilibili = (url: string) => invoke('navigate_bilibili_webview', { url });
export const closeBilibiliWebview = () => invoke('close_bilibili_webview');
export const focusBilibiliWebview = () => invoke('open_bilibili_webview', {});
/**
 * 上报播放区 DOM 矩形（CSS 逻辑像素），Rust 据此校准嵌入子 webview 的位置/尺寸。
 * width/height 为 0（窄窗口 .player 折叠）时触发 Rust 隐藏子 webview。
 * 矩形在 Rust 侧缓存：子 webview 未创建时仅缓存，创建时套用，避免全窗闪现。
 */
export const setBilibiliBounds = (x: number, y: number, width: number, height: number) =>
  invoke<void>('set_bili_webview_bounds', { x, y, width, height });
