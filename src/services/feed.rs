use crate::error::{AppError, AppResult};
use crate::models::Show;
use crate::services::ffmpeg::format_duration;
use chrono::{DateTime, Utc};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;
use std::io::Cursor;

#[derive(Debug, Clone)]
pub struct FeedEpisode {
    pub uuid: String,
    pub title: String,
    pub description: String,
    pub source_url: String,
    pub published_at: i64,
    pub updated_at: i64,
    pub audio_url: String,
    pub audio_bytes: i64,
    pub audio_duration_s: i64,
    pub explicit: bool,
}

pub fn render(
    show: &Show,
    episodes: &[FeedEpisode],
    public_base_url: &str,
    last_build: i64,
) -> AppResult<String> {
    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);
    start(
        &mut writer,
        "rss",
        &[
            ("version", "2.0"),
            ("xmlns:itunes", "http://www.itunes.com/dtds/podcast-1.0.dtd"),
            ("xmlns:atom", "http://www.w3.org/2005/Atom"),
        ],
    )?;
    start(&mut writer, "channel", &[])?;
    text(&mut writer, "title", &show.title)?;
    text(
        &mut writer,
        "link",
        if show.website_url.is_empty() {
            public_base_url
        } else {
            &show.website_url
        },
    )?;
    text(&mut writer, "description", &show.description)?;
    text(&mut writer, "language", &show.language)?;
    text(&mut writer, "copyright", &format!("© {}", show.owner_name))?;
    text(&mut writer, "lastBuildDate", &rfc2822(last_build))?;
    text(&mut writer, "generator", "Tube2Pod")?;
    let self_url = format!("{public_base_url}/feed/{}.xml", show.feed_token);
    empty(
        &mut writer,
        "atom:link",
        &[
            ("href", &self_url),
            ("rel", "self"),
            ("type", "application/rss+xml"),
        ],
    )?;
    text(&mut writer, "itunes:author", &show.author)?;
    text(&mut writer, "itunes:summary", &show.description)?;
    start(&mut writer, "itunes:owner", &[])?;
    text(&mut writer, "itunes:name", &show.owner_name)?;
    text(&mut writer, "itunes:email", &show.owner_email)?;
    end(&mut writer, "itunes:owner")?;
    empty(
        &mut writer,
        "itunes:image",
        &[("href", &show.cover_url(public_base_url))],
    )?;
    empty(&mut writer, "itunes:category", &[("text", &show.category)])?;
    text(
        &mut writer,
        "itunes:explicit",
        if show.explicit { "true" } else { "false" },
    )?;
    text(&mut writer, "itunes:type", "episodic")?;

    for episode in episodes {
        start(&mut writer, "item", &[])?;
        text(&mut writer, "title", &episode.title)?;
        text(&mut writer, "description", &episode.description)?;
        text(&mut writer, "link", &episode.source_url)?;
        start(&mut writer, "guid", &[("isPermaLink", "false")])?;
        writer.write_event(Event::Text(BytesText::new(&format!(
            "tube2pod:{}",
            episode.uuid
        ))))?;
        end(&mut writer, "guid")?;
        text(&mut writer, "pubDate", &rfc2822(episode.published_at))?;
        empty(
            &mut writer,
            "enclosure",
            &[
                ("url", &episode.audio_url),
                ("length", &episode.audio_bytes.to_string()),
                ("type", "audio/mpeg"),
            ],
        )?;
        text(
            &mut writer,
            "itunes:duration",
            &format_duration(episode.audio_duration_s),
        )?;
        text(&mut writer, "itunes:author", &show.author)?;
        text(&mut writer, "itunes:summary", &episode.description)?;
        text(
            &mut writer,
            "itunes:explicit",
            if episode.explicit { "true" } else { "false" },
        )?;
        end(&mut writer, "item")?;
    }

    end(&mut writer, "channel")?;
    end(&mut writer, "rss")?;
    let bytes = writer.into_inner().into_inner();
    String::from_utf8(bytes).map_err(|e| AppError::Internal(e.into()))
}

fn start(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    attrs: &[(&str, &str)],
) -> AppResult<()> {
    let mut element = BytesStart::new(name);
    for (key, value) in attrs {
        element.push_attribute((*key, *value));
    }
    writer.write_event(Event::Start(element))?;
    Ok(())
}

fn empty(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    attrs: &[(&str, &str)],
) -> AppResult<()> {
    let mut element = BytesStart::new(name);
    for (key, value) in attrs {
        element.push_attribute((*key, *value));
    }
    writer.write_event(Event::Empty(element))?;
    Ok(())
}

fn text(writer: &mut Writer<Cursor<Vec<u8>>>, name: &str, value: &str) -> AppResult<()> {
    writer.write_event(Event::Start(BytesStart::new(name)))?;
    writer.write_event(Event::Text(BytesText::new(value)))?;
    writer.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

fn end(writer: &mut Writer<Cursor<Vec<u8>>>, name: &str) -> AppResult<()> {
    writer.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

pub fn rfc2822(ts: i64) -> String {
    DateTime::<Utc>::from_timestamp(ts, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).unwrap())
        .to_rfc2822()
}

pub fn etag(show: &Show, episodes: &[FeedEpisode], last_build: i64) -> String {
    let latest = episodes.iter().map(|ep| ep.updated_at).max().unwrap_or(0);
    format!(
        "\"{:x}-{:x}-{:x}-{:x}\"",
        last_build,
        episodes.len(),
        latest,
        show.cover_version
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Show;

    fn show() -> Show {
        Show {
            id: 1,
            title: "Công nghệ & Đời sống".into(),
            author: "Tác giả".into(),
            description: "Mô tả <hay> & thú vị".into(),
            category: "Technology".into(),
            owner_name: "Chủ show".into(),
            owner_email: "owner@example.com".into(),
            explicit: false,
            language: "vi".into(),
            website_url: "".into(),
            cover_path: Some("/data/cover.jpg".into()),
            cover_version: 2,
            cover_custom: false,
            feed_token: "token123".into(),
            feed_enabled: true,
            default_minutes: 10,
            auto_publish: true,
            voice_preset_json: "{}".into(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn episode() -> FeedEpisode {
        FeedEpisode {
            uuid: "uuid-1".into(),
            title: "Tập <1> & thử nghiệm".into(),
            description: "Tóm tắt\nhai dòng".into(),
            source_url: "https://www.youtube.com/watch?v=abc".into(),
            published_at: 1_700_000_000,
            updated_at: 1_700_000_100,
            audio_url: "https://pod.example/audio/token/uuid-1-1.mp3".into(),
            audio_bytes: 1234,
            audio_duration_s: 61,
            explicit: false,
        }
    }

    #[test]
    fn renders_itunes_feed_with_escaping_and_stable_guid() {
        let xml = render(
            &show(),
            &[episode()],
            "https://pod.example.ts.net:8443",
            1_700_000_100,
        )
        .unwrap();
        assert!(xml.contains("<title>Công nghệ &amp; Đời sống</title>"));
        assert!(xml.contains("tube2pod:uuid-1"));
        assert!(xml.contains("isPermaLink=\"false\""));
        assert!(xml.contains("length=\"1234\""));
        assert!(xml.contains("type=\"audio/mpeg\""));
        assert!(xml.contains("<itunes:duration>00:01:01</itunes:duration>"));
        assert!(xml.contains("https://pod.example.ts.net:8443/cover/token123-2.jpg"));
        assert!(xml.contains("https://pod.example.ts.net:8443/feed/token123.xml"));
    }

    #[test]
    fn empty_feed_is_still_valid_rss() {
        let xml = render(
            &show(),
            &[],
            "https://pod.example.ts.net:8443",
            1_700_000_100,
        )
        .unwrap();
        assert!(xml.contains("<rss"));
        assert!(xml.contains("<channel>"));
        assert!(!xml.contains("<item>"));
    }

    #[test]
    fn etag_changes_when_episode_changes() {
        let a = etag(&show(), &[episode()], 1_700_000_100);
        let mut changed = episode();
        changed.updated_at += 1;
        let b = etag(&show(), &[changed], 1_700_000_100);
        assert_ne!(a, b);
    }
}
