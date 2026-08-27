import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { ParsedItem } from '../types/playlist';

/**
 * 调用 Rust 解析适配器（§7），把列表页 HTML 解析为结构化项目 DTO。
 * 本步暂不被 UI 调用——HTML 获取由第 ③ 步 WebView 桥接提供。
 */
export function parseListHtml(sourceUrl: string, html: string): Promise<ParsedItem[]> {
  return invoke<ParsedItem[]>('parse_list_html', { sourceUrl, html });
}

export async function captureAndParse(sourceUrl: string): Promise<{ items: ParsedItem[]; listTitle: string | null }> {
  const requestId = crypto.randomUUID();
  let un: UnlistenFn | undefined;
  let unState: UnlistenFn | undefined;
  return new Promise((resolve, reject) => {
    const timer: ReturnType<typeof setTimeout> = setTimeout(() => { un?.(); unState?.(); reject(new Error('解析超时，请确认 WebView 已登录且列表页已加载')); }, 20000);
    listen<{ requestId: string; items: ParsedItem[]; error?: string; listTitle?: string }>('bilibili://parse-result', (e) => {
      if (e.payload.requestId !== requestId) return;
      clearTimeout(timer); un?.(); unState?.();
      e.payload.error ? reject(new Error(e.payload.error)) : resolve({ items: e.payload.items, listTitle: e.payload.listTitle ?? null });
    }).then((u) => (un = u));
    // 风控/登录拦截：B 站把未登录访问 302 到 passport.bilibili.com 验证页，
    // capture 脚本永远等不到列表 DOM。监听 page-state 的 guest 信号立即失败，不傻等 20s。
    listen<{ loginState: string; url: string }>('bilibili://page-state', (e) => {
      if (e.payload.loginState === 'guest') {
        clearTimeout(timer); un?.(); unState?.();
        reject(new Error('被 B 站风控拦截（未登录）。请先点「打开登录页」在播放区登录 B 站，再重新导入'));
      }
    }).then((u) => (unState = u));
    invoke('capture_list_html', { sourceUrl, requestId }).catch((err) => { clearTimeout(timer); un?.(); unState?.(); reject(err); });
  });
}
