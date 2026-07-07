// Rust agent core.
use base64::Engine;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, USER_AGENT};
use reqwest::Url;
use serde_json::Value;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

const FAVICON_MAX_BYTES: usize = 64 * 1024;
const FAVICON_HTML_MAX_BYTES: usize = 768 * 1024;
const FAVICON_TIMEOUT_MILLIS: u64 = 1_200;
const FAVICON_USER_AGENT: &str = "MyCopilot/0.1 favicon resolver";

pub(super) struct FaviconAsset {
    pub data_url: String,
    pub mime_type: String,
}

pub(super) struct FaviconFetcher {
    client: Option<Client>,
}

impl FaviconFetcher {
    pub fn new() -> Self {
        Self {
            client: favicon_client(),
        }
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self { client: None }
    }

    pub fn fetch(&self, result: &Value, page_url: &str) -> Option<FaviconAsset> {
        fetch_favicon_asset(self.client.as_ref()?, result, page_url)
    }
}

fn favicon_client() -> Option<Client> {
    Client::builder()
        .timeout(Duration::from_millis(FAVICON_TIMEOUT_MILLIS))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 {
                attempt.stop()
            } else if normalize_public_url(attempt.url().as_str()).is_none() {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .ok()
}

fn fetch_favicon_asset(client: &Client, result: &Value, page_url: &str) -> Option<FaviconAsset> {
    let page_url = normalize_public_url(page_url)?;

    for candidate in favicon_candidates(result, &page_url) {
        if let Some(asset) = fetch_favicon_url(client, &candidate) {
            return Some(asset);
        }
    }

    for candidate in fetch_html_favicon_candidates(client, &page_url) {
        if let Some(asset) = fetch_favicon_url(client, &candidate) {
            return Some(asset);
        }
    }

    None
}

fn favicon_candidates(result: &Value, page_url: &Url) -> Vec<Url> {
    let mut candidates = Vec::new();

    for key in ["favicon", "favicon_url", "faviconUrl"] {
        if let Some(candidate) = result
            .get(key)
            .and_then(Value::as_str)
            .and_then(|value| resolve_public_url(page_url, value))
        {
            push_unique_url(&mut candidates, candidate);
        }
    }

    if let Some(candidate) = origin_favicon_url(page_url) {
        push_unique_url(&mut candidates, candidate);
    }

    candidates
}

fn push_unique_url(candidates: &mut Vec<Url>, url: Url) {
    if !candidates.iter().any(|candidate| candidate == &url) {
        candidates.push(url);
    }
}

fn origin_favicon_url(page_url: &Url) -> Option<Url> {
    let origin = page_url.origin().ascii_serialization();
    normalize_public_url(&format!("{origin}/favicon.ico"))
}

fn resolve_public_url(base_url: &Url, value: &str) -> Option<Url> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let resolved = base_url.join(value).ok()?;
    normalize_public_url(resolved.as_str())
}

fn normalize_public_url(value: &str) -> Option<Url> {
    let url = Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }

    let host = url.host_str()?.trim().to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return None;
    }
    if host == "metadata.google.internal" {
        return None;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return None;
        }
    }

    Some(url)
}

fn fetch_favicon_url(client: &Client, url: &Url) -> Option<FaviconAsset> {
    let response = client
        .get(url.clone())
        .header(USER_AGENT, FAVICON_USER_AGENT)
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }

    let mime_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(normalize_favicon_mime_type)
        .or_else(|| favicon_mime_type_from_url(url))?;
    let bytes = response.bytes().ok()?;
    if bytes.is_empty() || bytes.len() > FAVICON_MAX_BYTES {
        return None;
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(FaviconAsset {
        data_url: format!("data:{mime_type};base64,{encoded}"),
        mime_type,
    })
}

fn fetch_html_favicon_candidates(client: &Client, page_url: &Url) -> Vec<Url> {
    let mut response = match client
        .get(page_url.clone())
        .header(USER_AGENT, FAVICON_USER_AGENT)
        .send()
    {
        Ok(response) => response,
        Err(_) => return Vec::new(),
    };
    if !response.status().is_success() {
        return Vec::new();
    }
    if !response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(is_html_content_type)
        .unwrap_or(true)
    {
        return Vec::new();
    }

    let mut bytes = Vec::new();
    let max_bytes = FAVICON_HTML_MAX_BYTES as u64;
    if response
        .by_ref()
        .take(max_bytes)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Vec::new();
    }
    let html = match std::str::from_utf8(&bytes) {
        Ok(html) => html,
        Err(_) => return Vec::new(),
    };

    html_favicon_candidates(html, page_url)
}

fn is_html_content_type(content_type: &str) -> bool {
    let mime_type = content_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();

    matches!(mime_type.as_str(), "text/html" | "application/xhtml+xml")
}

fn html_favicon_candidates(html: &str, page_url: &Url) -> Vec<Url> {
    let lower_html = html.to_ascii_lowercase();
    let mut candidates = Vec::new();
    let mut offset = 0;

    while let Some(relative_start) = lower_html[offset..].find("<link") {
        let tag_start = offset + relative_start;
        let Some(relative_end) = lower_html[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + relative_end + 1;
        let tag = &html[tag_start..tag_end];

        if html_link_rel_has_icon(tag) {
            if let Some(href) =
                html_attr_value(tag, "href").and_then(|href| resolve_public_url(page_url, &href))
            {
                push_unique_url(&mut candidates, href);
            }
        }

        offset = tag_end;
    }

    candidates
}

fn html_link_rel_has_icon(tag: &str) -> bool {
    html_attr_value(tag, "rel")
        .map(|rel| {
            rel.to_ascii_lowercase()
                .split_ascii_whitespace()
                .any(|token| {
                    matches!(
                        token,
                        "icon" | "apple-touch-icon" | "apple-touch-icon-precomposed"
                    )
                })
        })
        .unwrap_or(false)
}

fn html_attr_value(tag: &str, attr_name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let attr_name = attr_name.to_ascii_lowercase();
    let mut index = 0;

    while index < bytes.len() {
        while index < bytes.len() && !is_html_attr_name_char(bytes[index]) {
            index += 1;
        }
        let name_start = index;
        while index < bytes.len() && is_html_attr_name_char(bytes[index]) {
            index += 1;
        }
        if name_start == index {
            break;
        }

        let name = tag[name_start..index].to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'=' {
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }

        let value = if bytes[index] == b'\'' || bytes[index] == b'"' {
            let quote = bytes[index];
            index += 1;
            let value_start = index;
            while index < bytes.len() && bytes[index] != quote {
                index += 1;
            }
            let value = tag[value_start..index].to_string();
            if index < bytes.len() {
                index += 1;
            }
            value
        } else {
            let value_start = index;
            while index < bytes.len() && !bytes[index].is_ascii_whitespace() && bytes[index] != b'>'
            {
                index += 1;
            }
            tag[value_start..index].to_string()
        };

        if name == attr_name {
            return Some(decode_html_attr_value(&value));
        }
    }

    None
}

fn is_html_attr_name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':')
}

fn decode_html_attr_value(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&#038;", "&")
        .replace("&#38;", "&")
        .trim()
        .to_string()
}

fn normalize_favicon_mime_type(content_type: &str) -> Option<String> {
    let mime_type = content_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        mime_type.as_str(),
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/x-icon"
            | "image/vnd.microsoft.icon"
    ) {
        return Some(if mime_type == "image/vnd.microsoft.icon" {
            "image/x-icon".to_string()
        } else {
            mime_type
        });
    }

    None
}

fn favicon_mime_type_from_url(url: &Url) -> Option<String> {
    let path = url.path().to_ascii_lowercase();
    let mime_type = if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".gif") {
        "image/gif"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else {
        return None;
    };

    Some(mime_type.to_string())
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_blocked_ipv4(ip),
        IpAddr::V6(ip) => is_blocked_ipv6(ip),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.octets()[0] == 0
        || ip.octets()[0] == 127
        || ip.octets()[0] == 169 && ip.octets()[1] == 254
        || ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1])
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_multicast()
        || ip.is_unspecified()
        || is_unique_local_ipv6(ip)
        || is_unicast_link_local_ipv6(ip)
}

fn is_unique_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_unicast_link_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_favicon_candidates_from_html_links() {
        let page_url = Url::parse(
            "https://understandingwar.org/research/russia-ukraine/russian-offensive-campaign-assessment-july-1-2026",
        )
        .unwrap();
        let candidates = html_favicon_candidates(
            r#"
            <link rel="canonical" href="https://understandingwar.org/research/russia-ukraine/russian-offensive-campaign-assessment-july-1-2026/" />
            <link rel="stylesheet" href="/theme.css" />
            <link rel="icon" href="https://understandingwar.org/wp-content/uploads/2024/10/cropped-ISW-Favicon-32x32.png" sizes="32x32" />
            <link rel="apple-touch-icon" href="/wp-content/uploads/2024/10/cropped-ISW-Favicon-180x180.png" />
            "#,
            &page_url,
        );

        let candidate_urls = candidates.iter().map(Url::as_str).collect::<Vec<_>>();
        assert_eq!(
            candidate_urls,
            vec![
                "https://understandingwar.org/wp-content/uploads/2024/10/cropped-ISW-Favicon-32x32.png",
                "https://understandingwar.org/wp-content/uploads/2024/10/cropped-ISW-Favicon-180x180.png",
            ],
        );
    }
}
