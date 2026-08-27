const BILIBILI_HOSTS = new Set(['www.bilibili.com', 'bilibili.com']);

/**
 * 客户端即时预检：仅做快速格式校验（粘贴时免 IPC 往返）。
 * 权威校验与列表解析由 Rust `parse_list_html` 命令完成（§7 解析适配器位于 Rust Core）。
 */
export function validateListUrl(value: string): URL {
  const url = new URL(value);
  if (url.protocol !== 'https:' || !BILIBILI_HOSTS.has(url.hostname) || !(url.pathname.startsWith('/list/') || url.pathname.startsWith('/video/'))) {
    throw new Error('请输入有效的 Bilibili 列表或视频地址');
  }
  return url;
}
