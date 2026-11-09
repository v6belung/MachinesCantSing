//! Full artist credits for a Spotify track, from its public web page.
//!
//! Spotify's desktop client only reports the *primary* artist to the OS (one-element
//! `xesam:artist` over MPRIS, a single string over Windows SMTC), so a collab like "Eiden XII,
//! Seraphina Rossi" arrives as just "Eiden XII". The public `open.spotify.com/track/<id>` page
//! (no login, no API key) lists every artist in its `<head>`:
//!
//! ```html
//! <meta name="music:musician_description" content="Eiden XII, Seraphina Rossi"/>
//! <meta name="music:musician" content="https://open.spotify.com/artist/7aN8..."/>
//! <meta name="music:musician" content="https://open.spotify.com/artist/2loN..."/>
//! ```
//!
//! The description is comma-joined, and a real artist name can contain a comma -- which is
//! exactly why splitting was dropped on Windows. Here, the split is only trusted when it yields
//! as many names as there are `music:musician` tags (one per artist); otherwise the lookup
//! gives up and the app falls back to the primary artist alone.
//!
//! Only usable where the OS reports a track ID (Linux/MPRIS). This scrapes a web page, not an
//! API, so every failure mode degrades to "primary artist only", never to an error state.

use std::time::Duration;

const USER_AGENT: &str = concat!("NowPlayingFlagger/", env!("CARGO_PKG_VERSION"));
/// The artist tags sit in `<head>`; the full page is ~300KB of mostly script. Stop reading at
/// `</head>`, with a hard cap in case the markup changes and it never appears.
const MAX_HEAD_BYTES: usize = 512 * 1024;

pub struct SpotifyPageClient {
    http: reqwest::Client,
}

impl SpotifyPageClient {
    pub fn new() -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { http })
    }

    /// `Ok(None)` = the page loaded but didn't yield a trustworthy artist list.
    pub async fn track_artists(&self, track_id: &str) -> anyhow::Result<Option<Vec<String>>> {
        anyhow::ensure!(is_valid_track_id(track_id), "invalid track id {track_id:?}");
        let url = format!("https://open.spotify.com/track/{track_id}");
        let mut resp = self.http.get(&url).send().await?.error_for_status()?;

        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            let search_from = buf.len().saturating_sub(b"</head>".len());
            buf.extend_from_slice(&chunk);
            if contains(&buf[search_from..], b"</head>") || buf.len() >= MAX_HEAD_BYTES {
                break;
            }
        }

        Ok(parse_track_artists(&String::from_utf8_lossy(&buf)))
    }
}

/// Spotify IDs are 22-character base62; anything else never goes into a URL.
pub fn is_valid_track_id(id: &str) -> bool {
    id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

pub fn parse_track_artists(html: &str) -> Option<Vec<String>> {
    let metas = meta_tags(html);
    let musician_count = named(&metas, "music:musician").count();
    let description = named(&metas, "music:musician_description").next()?;
    let names: Vec<String> = description
        .split(", ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    (musician_count > 0 && names.len() == musician_count).then_some(names)
}

fn named<'a>(metas: &'a [(String, String)], name: &'a str) -> impl Iterator<Item = &'a str> {
    metas
        .iter()
        .filter(move |(n, _)| n == name)
        .map(|(_, content)| content.as_str())
}

/// `(name-or-property, content)` for every `<meta>` tag, entity-decoded. A deliberately small
/// scanner rather than an HTML parser dependency: it only has to handle Spotify's own
/// well-formed, double-quoted meta tags.
fn meta_tags(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<meta ") {
        rest = &rest[start + "<meta ".len()..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        rest = &rest[end..];

        let (mut key, mut content) = (None, None);
        for (attr, value) in attributes(tag) {
            match attr {
                "name" | "property" => key = Some(value),
                "content" => content = Some(value),
                _ => {}
            }
        }
        if let (Some(key), Some(content)) = (key, content) {
            out.push((decode_entities(key), decode_entities(content)));
        }
    }
    out
}

/// `attr="value"` pairs from inside one tag.
fn attributes(tag: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = tag;
    while let Some(eq) = rest.find("=\"") {
        let attr = rest[..eq]
            .rsplit([' ', '\t', '\n'])
            .next()
            .unwrap_or("")
            .trim();
        let after = &rest[eq + 2..];
        let Some(close) = after.find('"') else { break };
        out.push((attr, &after[..close]));
        rest = &after[close + 1..];
    }
    out
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest.find(';').filter(|&semi| semi <= 10).and_then(|semi| {
            let entity = &rest[1..semi];
            let ch = match entity {
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "lt" => Some('<'),
                "gt" => Some('>'),
                _ => entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            ch.map(|c| (c, semi))
        });
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(description: &str, musicians: usize) -> String {
        let mut html = String::from(
            r#"<html><head><meta property="og:title" content="Mi Mi Mi - Rock Version"/>"#,
        );
        for i in 0..musicians {
            html.push_str(&format!(
                r#"<meta name="music:musician" content="https://open.spotify.com/artist/{i}"/>"#
            ));
        }
        html.push_str(&format!(
            r#"<meta name="music:musician_description" content="{description}"/></head>"#
        ));
        html
    }

    #[test]
    fn two_artists() {
        assert_eq!(
            parse_track_artists(&page("Eiden XII, Seraphina Rossi", 2)),
            Some(vec!["Eiden XII".to_string(), "Seraphina Rossi".to_string()])
        );
    }

    #[test]
    fn single_artist() {
        assert_eq!(
            parse_track_artists(&page("Queen", 1)),
            Some(vec!["Queen".to_string()])
        );
    }

    #[test]
    fn comma_inside_a_name_is_rejected_not_split() {
        // One artist whose name has a comma: 2 split names vs 1 musician tag.
        assert_eq!(parse_track_artists(&page("Earth, Wind & Fire", 1)), None);
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(
            parse_track_artists(&page("Simon &amp; Garfunkel, Beyonc&#xE9;", 2)),
            Some(vec!["Simon & Garfunkel".to_string(), "Beyoncé".to_string()])
        );
    }

    #[test]
    fn missing_tags_yield_nothing() {
        assert_eq!(parse_track_artists("<html><head></head></html>"), None);
        assert_eq!(parse_track_artists(&page("A, B", 0)), None);
    }

    #[test]
    fn track_id_validation() {
        assert!(is_valid_track_id("4Vw6c5OhtMZY5roKr7EQ0B"));
        assert!(!is_valid_track_id("../../evil"));
        assert!(!is_valid_track_id("4Vw6c5OhtMZY5roKr7EQ0B?x=1"));
    }

    #[test]
    fn decode_leaves_bare_ampersands_alone() {
        assert_eq!(
            decode_entities("A & B &unknown; &#65;"),
            "A & B &unknown; A"
        );
    }
}
