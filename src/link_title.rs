//! 関連資料の URL から、リンク先のページの名前（<title>）を取る。
//! 取れない（ログインが要る・HTML でない・つながらない）ときは None を返し、呼び出し側でホスト名とパスを使う。

use std::time::Duration;

/// 読むのは先頭のこの量まで（<title> は普通 <head> の中にある）
const MAX_BYTES: usize = 512 * 1024;

pub async fn fetch_title(url: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("my_todo/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let mut resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let is_html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("html"));
    if !is_html {
        return None;
    }
    let mut body = Vec::new();
    while body.len() < MAX_BYTES {
        match resp.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            Ok(None) | Err(_) => break,
        }
    }
    extract_title(&String::from_utf8_lossy(&body))
}

/// HTML から <title>（なければ og:title）を取り出す
pub fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let from_title = lower.find("<title").and_then(|start| {
        let open_end = start + lower[start..].find('>')? + 1;
        let close = open_end + lower[open_end..].find("</title")?;
        Some(html[open_end..close].to_string())
    });
    let from_og = || {
        let at = lower.find("og:title")?;
        let tag_start = lower[..at].rfind('<')?;
        let tag_end = at + lower[at..].find('>')?;
        let tag = &html[tag_start..tag_end];
        let c = tag.to_ascii_lowercase().find("content=")? + "content=".len();
        let quote = tag[c..].chars().next()?;
        let rest = &tag[c + quote.len_utf8()..];
        Some(rest[..rest.find(quote)?].to_string())
    };
    let raw = from_title
        .filter(|t| !t.trim().is_empty())
        .or_else(from_og)?;
    let text = decode_entities(&raw)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // 文字化け（UTF-8 以外のページ）なら使わない
    (!text.is_empty() && !text.contains('\u{FFFD}')).then_some(text)
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        let Some(semi) = after.find(';').filter(|&i| i <= 10) else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let name = &after[1..semi];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_and_og_title() {
        assert_eq!(
            extract_title(
                "<html><head><TITLE>\n  設計書 &amp; 仕様 &#x2014; Wiki\n</TITLE></head>"
            )
            .as_deref(),
            Some("設計書 & 仕様 \u{2014} Wiki")
        );
        assert_eq!(
            extract_title(r#"<meta property="og:title" content="会議メモ"><title></title>"#)
                .as_deref(),
            Some("会議メモ")
        );
        assert_eq!(extract_title("<p>no title</p>"), None);
        assert_eq!(
            decode_entities("a & b &unknown; &#65;"),
            "a & b &unknown; A"
        );
    }
}
