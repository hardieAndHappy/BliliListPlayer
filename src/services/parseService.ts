import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { ParsedItem } from '../types/playlist';
import { getPageAccessErrorMessage } from './bilibiliPageState';

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
    const timer: ReturnType<typeof setTimeout> = setTimeout(() => { un?.(); unState?.(); reject(new Error('解析超时，请确认网络正常且列表页可访问')); }, 20000);
    listen<{ requestId: string; items: ParsedItem[]; error?: string; listTitle?: string }>('bilibili://parse-result', (e) => {
      if (e.payload.requestId !== requestId) return;
      clearTimeout(timer); un?.(); unState?.();
      e.payload.error ? reject(new Error(e.payload.error)) : resolve({ items: e.payload.items, listTitle: e.payload.listTitle ?? null });
    }).then((u) => (un = u));
    // 只有 B 站实际跳转到 Passport 登录/验证页时才早退；公开页面和旧版 guest 状态继续抓取。
    listen<{ loginState: string; url: string }>('bilibili://page-state', (e) => {
      const message = getPageAccessErrorMessage(e.payload.loginState);
      if (!message) return;
      clearTimeout(timer); un?.(); unState?.();
      reject(new Error(message));
    }).then((u) => (unState = u));
    invoke('capture_list_html', { sourceUrl, requestId }).catch((err) => { clearTimeout(timer); un?.(); unState?.(); reject(err); });
  });
}
