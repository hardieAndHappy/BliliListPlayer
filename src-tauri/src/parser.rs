//! Bilibili 列表解析适配器（纯函数，无 I/O）。
//!
//! 依据规范 §7：解析器与 UI 解耦，以结构化项目 DTO 交互，页面结构变化只影响本适配器。
//! 依据规范 §5.1：只提取列表页中指向具体视频的页面链接，规范化为 BV/AV 标识；
//! 禁止提取 playurl、音视频流地址、签名参数或执行下载（§5.1 line 78）。
//! 本模块不抓取网络——HTML 由调用方（第 ③ 步的 WebView 桥接）喂入。

use crate::model::{ItemStatus, ParsedItem};
use std::collections::HashSet;

const BILIBILI_HOSTS: [&str; 2] = ["www.bilibili.com", "bilibili.com"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    EmptyUrl,
    InvalidScheme,
    InvalidHost,
    InvalidPath,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::EmptyUrl => write!(f, "请输入有效的 Bilibili 列表或视频地址"),
            ParseError::InvalidScheme => write!(f, "列表地址必须为 https://"),
            ParseError::InvalidHost => write!(f, "仅支持 Bilibili 官方列表地址"),
            ParseError::InvalidPath => write!(f, "地址路径需为 /list/<id> 或 /video/<id> 形式"),
        }
    }
}

impl std::error::Error for ParseError {}

/// 校验并规范化 Bilibili 列表或视频来源 URL。
/// 接受 `/list/<id>` 与 `/video/<id>`，规范化时补 `www.`、去除 query/fragment。
pub fn validate_list_url(url: &str) -> Result<String, ParseError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(ParseError::EmptyUrl);
    }
    let rest = trimmed
        .strip_prefix("https://")
        .ok_or(ParseError::InvalidScheme)?;
    let (host, after_host) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if !BILIBILI_HOSTS.contains(&host) {
        return Err(ParseError::InvalidHost);
    }
    let path_end = after_host
        .find(|c| c == '?' || c == '#')
        .unwrap_or(after_host.len());
    let path = &after_host[..path_end];
    if !(path.starts_with("/list/") || path.starts_with("/video/")) {
        return Err(ParseError::InvalidPath);
    }
    Ok(format!("https://www.bilibili.com{}", path))
}

/// 从路径或 URL 中提取并规范化的视频标识：BV（大小写敏感）优先，否则 av+数字（小写）。
pub fn normalize_video_id(path_or_url: &str) -> Option<String> {
    if let Some(pos) = path_or_url.find("BV") {
        let run: String = path_or_url[pos + 2..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if !run.is_empty() {
            return Some(format!("BV{}", run));
        }
    }
    let lower = path_or_url.to_ascii_lowercase();
    if let Some(pos) = lower.find("av") {
        let run: String = lower[pos + 2..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !run.is_empty() {
            return Some(format!("av{}", run));
        }
    }
    None
}

/// 解析列表页 HTML，提取指向具体视频页面的链接并组装为 DTO。
/// - 仅接受 Bilibili 主机 + `/video/<slug>` 路径（playurl/流地址/签名按构造排除）。
/// - 按 id 小写在来源内去重；first-seen 序。
/// - 可规范化的项目（BV/AV）→ 有标题记 Playable，无标题记 Pending。
/// - 不可规范化的项目（如 `/video/xyz`）保留并记 Invalid，不阻塞其他项目（§5.1 line 191）。
pub fn parse_list_html(source_url: &str, html: &str) -> Vec<ParsedItem> {
    if validate_list_url(source_url).is_err() {
        return Vec::new();
    }
    let mut items = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (href, text) in iter_anchors(html) {
        let Some((host, path)) = resolve_url(&href) else {
            continue;
        };
        if !BILIBILI_HOSTS.contains(&host.as_str()) {
            continue;
        }
        let Some(slug) = path.strip_prefix("/video/") else {
            continue;
        };
        let slug = slug.split(['/', '?', '#']).next().unwrap_or(slug);
        if slug.is_empty() {
            continue;
        }
        match normalize_video_id(slug) {
            Some(id) => {
                if !seen.insert(id.to_lowercase()) {
                    continue;
                }
                let status = if text.is_empty() {
                    ItemStatus::Pending
                } else {
                    ItemStatus::Playable
                };
                let url = format!("https://www.bilibili.com/video/{}", id);
                items.push(ParsedItem {
                    id,
                    title: text,
                    url,
                    cover_url: None,
                    author: None,
                    status,
                    duration_secs: None,
                });
            }
            None => {
                if !seen.insert(slug.to_lowercase()) {
                    continue;
                }
                items.push(ParsedItem {
                    id: slug.to_string(),
                    title: text,
                    url: format!("https://www.bilibili.com/video/{}", slug),
                    cover_url: None,
                    author: None,
                    status: ItemStatus::Invalid,
                    duration_secs: None,
                });
            }
        }
    }
    items
}

/// 将新解析项目按 BV/AV 标识（小写）对已有 id 去重（§5.1 line 83）。
pub fn dedup_items(new: Vec<ParsedItem>, existing_ids: &[String]) -> Vec<ParsedItem> {
    let existing: HashSet<String> = existing_ids.iter().map(|s| s.to_lowercase()).collect();
    new.into_iter()
        .filter(|item| !existing.contains(&item.id.to_lowercase()))
        .collect()
}

// ---- HTML 扫描辅助（std-only，无外部依赖）----

/// 迭代 `<a ...>text</a>`，提取 `(href, 纯文本)`。
fn iter_anchors(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(open_rel) = rest.find("<a") {
        let after = &rest[open_rel + 2..];
        // `<a` 后须为空白或 `>`，排除 `<abbr>`/`<address>` 等
        match after.chars().next() {
            Some(c) if c.is_whitespace() || c == '>' => {}
            _ => {
                rest = &rest[open_rel + 2..];
                continue;
            }
        }
        let Some(gt_rel) = after.find('>') else { break };
        let opening = &after[..=gt_rel];
        let href = extract_attr(opening, "href").map(str::to_string);
        let inner_start = &after[gt_rel + 1..];
        let Some(close_rel) = inner_start.find("</a>") else { break };
        let inner = &inner_start[..close_rel];
        let text = strip_tags(inner).trim().to_string();
        if let Some(h) = href {
            out.push((h, text));
        }
        let consumed = open_rel + 2 + (gt_rel + 1) + (close_rel + 4);
        rest = &rest[consumed..];
    }
    out
}

/// 从开标签中提取属性值（处理单/双引号与无引号形式）。
fn extract_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let bytes = tag.as_bytes();
    let nb = name.as_bytes();
    let mut i = 0;
    while i + nb.len() <= bytes.len() {
        if &bytes[i..i + nb.len()] == nb {
            let before_ok = i == 0 || {
                let c = bytes[i - 1] as char;
                c.is_whitespace() || c == '>' || c == '/'
            };
            let mut j = i + nb.len();
            while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                j += 1;
            }
            if before_ok && j < bytes.len() && bytes[j] == b'=' {
                let mut k = j + 1;
                while k < bytes.len() && (bytes[k] as char).is_whitespace() {
                    k += 1;
                }
                if k < bytes.len() && (bytes[k] == b'"' || bytes[k] == b'\'') {
                    let quote = bytes[k];
                    let start = k + 1;
                    let end = bytes[start..]
                        .iter()
                        .position(|&b| b == quote)
                        .map(|p| start + p)
                        .unwrap_or(bytes.len());
                    return Some(&tag[start..end]);
                }
                let start = k;
                let end = bytes[start..]
                    .iter()
                    .position(|&b| (b as char).is_whitespace() || b == b'>')
                    .map(|p| start + p)
                    .unwrap_or(bytes.len());
                return Some(&tag[start..end]);
            }
        }
        i += 1;
    }
    None
}

/// 移除标签，保留纯文本。
fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(c);
        }
    }
    out
}

/// 将 href 解析为 `(host, path)`（仅 https、Bilibili 友好）。
/// 处理 `//host/path`、`/path`、`https://host/path`；拒绝 `http://`；跳过 `#` 与带 scheme 的非 http(s) 链接。
fn resolve_url(href: &str) -> Option<(String, String)> {
    let href = href.trim();
    let rest = if let Some(r) = href.strip_prefix("https://") {
        r
    } else if let Some(r) = href.strip_prefix("//") {
        r
    } else if href.starts_with("http://") {
        return None;
    } else if href.starts_with('/') {
        return Some(("www.bilibili.com".to_string(), href.to_string()));
    } else if href.starts_with('#') || href.contains(':') {
        return None;
    } else {
        return Some(("www.bilibili.com".to_string(), format!("/{}", href)));
    };
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let path = path.split(['?', '#']).next().unwrap_or(&path).to_string();
    Some((host.to_string(), path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(id: &str, title: &str, status: ItemStatus) -> ParsedItem {
        ParsedItem {
            id: id.to_string(),
            title: title.to_string(),
            url: format!("https://www.bilibili.com/video/{}", id),
            cover_url: None,
            author: None,
            status,
            duration_secs: None,
        }
    }

    // ---- validate_list_url ----

    #[test]
    fn validate_list_url_accepts_canonical() {
        assert_eq!(
            validate_list_url("https://www.bilibili.com/list/12853451").unwrap(),
            "https://www.bilibili.com/list/12853451"
        );
    }

    #[test]
    fn validate_list_url_normalizes_bare_host() {
        assert_eq!(
            validate_list_url("https://bilibili.com/list/1?x=1#frag").unwrap(),
            "https://www.bilibili.com/list/1"
        );
    }

    #[test]
    fn validate_list_url_rejects_non_bilibili() {
        assert_eq!(
            validate_list_url("https://example.com/list/1").unwrap_err(),
            ParseError::InvalidHost
        );
    }

    #[test]
    fn validate_list_url_rejects_http() {
        assert_eq!(
            validate_list_url("http://www.bilibili.com/list/1").unwrap_err(),
            ParseError::InvalidScheme
        );
    }

    #[test]
    fn validate_list_url_rejects_non_list_path() {
        assert_eq!(
            validate_list_url("https://www.bilibili.com/watchlater").unwrap_err(),
            ParseError::InvalidPath
        );
    }

    #[test]
    fn validate_list_url_accepts_video_page_with_query() {
        assert_eq!(
            validate_list_url("https://www.bilibili.com/video/BV1zK4y1F7NP/?spm_id_from=333.1387.0.0").unwrap(),
            "https://www.bilibili.com/video/BV1zK4y1F7NP/"
        );
    }

    #[test]
    fn validate_list_url_rejects_empty() {
        assert_eq!(validate_list_url("").unwrap_err(), ParseError::EmptyUrl);
        assert_eq!(validate_list_url("   ").unwrap_err(), ParseError::EmptyUrl);
    }

    // ---- normalize_video_id ----

    #[test]
    fn normalize_video_id_bv() {
        assert_eq!(
            normalize_video_id("/video/BV1abc123"),
            Some("BV1abc123".to_string())
        );
    }

    #[test]
    fn normalize_video_id_av() {
        assert_eq!(normalize_video_id("/video/av42"), Some("av42".to_string()));
        assert_eq!(normalize_video_id("/video/AV42"), Some("av42".to_string()));
    }

    #[test]
    fn normalize_video_id_none() {
        assert_eq!(normalize_video_id("/list/1"), None);
        assert_eq!(normalize_video_id("/video/xyz"), None);
    }

    // ---- parse_list_html ----

    #[test]
    fn parse_list_html_extracts_video_pages() {
        let html = r#"<a href="/video/BV1abc">one</a><a href="https://www.bilibili.com/video/av42">two</a>"#;
        let items = parse_list_html("https://www.bilibili.com/list/12853451", html);
        assert_eq!(
            items,
            vec![
                parsed("BV1abc", "one", ItemStatus::Playable),
                parsed("av42", "two", ItemStatus::Playable),
            ]
        );
    }

    #[test]
    fn parse_list_html_dedups_within_source() {
        let html = r#"<a href="/video/BV1abc">one</a><a href="/video/BV1abc">dup</a>"#;
        let items = parse_list_html("https://www.bilibili.com/list/1", html);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "BV1abc");
    }

    #[test]
    fn parse_list_html_excludes_stream_urls() {
        let html = r#"<a href="/video/BV1abc">one</a><a href="//xy.com/playurl?sign=x">stream</a>"#;
        let items = parse_list_html("https://www.bilibili.com/list/1", html);
        assert_eq!(items, vec![parsed("BV1abc", "one", ItemStatus::Playable)]);
    }

    #[test]
    fn parse_list_html_marks_unparseable_invalid() {
        let html = r#"<a href="/video/BV1abc">one</a><a href="/video/xyz">bad</a>"#;
        let items = parse_list_html("https://www.bilibili.com/list/1", html);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].status, ItemStatus::Playable);
        assert_eq!(items[1].id, "xyz");
        assert_eq!(items[1].status, ItemStatus::Invalid);
    }

    #[test]
    fn parse_list_html_invalid_source_returns_empty() {
        let html = r#"<a href="/video/BV1abc">one</a>"#;
        assert!(parse_list_html("https://example.com/list/1", html).is_empty());
    }

    #[test]
    fn parse_list_html_pending_when_no_title() {
        let html = r#"<a href="/video/BV1abc"></a>"#;
        let items = parse_list_html("https://www.bilibili.com/list/1", html);
        assert_eq!(items, vec![parsed("BV1abc", "", ItemStatus::Pending)]);
    }

    // ---- dedup_items ----

    #[test]
    fn dedup_items_drops_existing() {
        let new = vec![
            parsed("BV1", "a", ItemStatus::Playable),
            parsed("BV2", "b", ItemStatus::Playable),
        ];
        let result = dedup_items(new, &["BV1".to_string()]);
        assert_eq!(result, vec![parsed("BV2", "b", ItemStatus::Playable)]);
    }

    #[test]
    fn dedup_items_case_insensitive() {
        let new = vec![parsed("bv1", "a", ItemStatus::Playable)];
        let result = dedup_items(new, &["BV1".to_string()]);
        assert!(result.is_empty());
    }
}
