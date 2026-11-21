use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::models::supported_language;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoId {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaptionTrack {
    pub ext: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Metadata {
    pub title: String,
    pub description: Option<String>,
    #[serde(default)]
    pub formats: Vec<AudioFormat>,
    pub channel: Option<String>,
    pub duration: Option<i64>,
    pub upload_date: Option<String>,
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub is_live: bool,
    #[serde(default)]
    pub subtitles: HashMap<String, Vec<CaptionTrack>>,
    #[serde(default)]
    pub automatic_captions: HashMap<String, Vec<CaptionTrack>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AudioFormat {
    pub format_id: String,
    pub acodec: Option<String>,
    pub vcodec: Option<String>,
    pub language: Option<String>,
    pub language_preference: Option<i32>,
    pub format_note: Option<String>,
    pub audio_track_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrack {
    pub key: String,
    pub format_id: String,
    pub source_language: Option<String>,
    pub is_default: bool,
    pub is_original: bool,
}

pub fn audio_tracks(meta: &Metadata) -> Vec<AudioTrack> {
    let mut tracks: Vec<(AudioTrack, bool, bool)> = Vec::new();
    for format in &meta.formats {
        if format.format_id.is_empty() || matches!(format.acodec.as_deref(), None | Some("none")) {
            continue;
        }
        let note = format.format_note.as_deref().unwrap_or("");
        let label = note.split(',').next().unwrap_or("").trim();
        let label = label.strip_suffix(" (default)").unwrap_or(label).trim();
        let label = label.strip_suffix(" (original)").unwrap_or(label).trim();
        let label = if matches!(label, "tiny" | "low" | "medium" | "high" | "") {
            ""
        } else {
            label
        };
        let identifiable = format
            .audio_track_id
            .as_deref()
            .is_some_and(|id| !id.is_empty())
            || (format.language.is_some() && !label.is_empty());
        let key = if let Some(id) = format.audio_track_id.as_deref().filter(|id| !id.is_empty()) {
            format!("id:{id}")
        } else {
            // yt-dlp exposes the display name and language, but not YouTube's track ID.
            format!(
                "metadata:{}",
                serde_json::to_string(&(format.language.as_deref(), label)).unwrap()
            )
        };
        let lower_note = note.to_ascii_lowercase();
        let is_default = lower_note.contains("(default)") || format.language_preference == Some(5);
        let is_original = lower_note.contains("original") || format.language_preference == Some(10);
        let audio_only = format.vcodec.as_deref() == Some("none");
        if let Some((track, existing_audio_only, _)) =
            tracks.iter_mut().find(|(track, _, _)| track.key == key)
        {
            track.is_default |= is_default;
            track.is_original |= is_original;
            if audio_only && !*existing_audio_only {
                track.format_id.clone_from(&format.format_id);
                *existing_audio_only = true;
            }
        } else {
            tracks.push((
                AudioTrack {
                    key,
                    format_id: format.format_id.clone(),
                    source_language: format.language.clone(),
                    is_default,
                    is_original,
                },
                audio_only,
                identifiable,
            ));
        }
    }
    // Unlabelled formats cannot be mapped to a particular track in a multi-track source.
    if tracks.len() > 1 {
        tracks.retain(|(_, _, identifiable)| *identifiable);
    }
    tracks.sort_by_key(|(track, _, _)| !track.is_default);
    tracks.into_iter().map(|(track, _, _)| track).collect()
}

#[derive(Debug, Clone)]
pub struct Caption {
    pub lang: String,
    pub ext: String,
    pub automatic: bool,
}

pub fn validate_url(input: &str) -> AppResult<VideoId> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(AppError::BadRequest("Chỉ nhận link video YouTube.".into()));
    }
    let url = url::Url::parse(raw)
        .map_err(|_| AppError::BadRequest("Chỉ nhận link video YouTube.".into()))?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = matches!(
        host.as_str(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be"
    );
    if !allowed {
        return Err(AppError::BadRequest("Chỉ nhận link video YouTube.".into()));
    }
    if url.path().starts_with("/playlist") || url.path().starts_with("/@") {
        return Err(AppError::BadRequest(
            "Chỉ nhận một video, không nhận playlist/kênh.".into(),
        ));
    }
    let id = if host == "youtu.be" {
        url.path()
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("")
            .to_string()
    } else if url.path().starts_with("/shorts/") {
        url.path()
            .trim_start_matches("/shorts/")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string()
    } else if url.path() == "/watch" {
        url.query_pairs()
            .find(|(k, _)| k == "v")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    } else {
        String::new()
    };
    if !is_video_id(&id) {
        return Err(AppError::BadRequest("Chỉ nhận link video YouTube.".into()));
    }
    Ok(VideoId { id })
}

fn is_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

async fn run_dump(cfg: &Config, video: &VideoId) -> AppResult<Metadata> {
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new(&cfg.ytdlp_path)
            .kill_on_drop(true)
            .args([
                "--dump-single-json",
                "--no-playlist",
                "--skip-download",
                &format!("https://www.youtube.com/watch?v={}", video.id),
            ])
            .env("HOME", cfg.data_dir.join("tmp/yt-dlp"))
            .env("XDG_CACHE_HOME", cfg.data_dir.join("tmp/yt-dlp/cache"))
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| AppError::Upstream("yt-dlp timed out".into()))?
    .map_err(|e| AppError::Internal(e.into()))?;

    if !output.status.success() {
        let tail = String::from_utf8_lossy(&output.stderr);
        let tail = tail.chars().take(4096).collect::<String>();
        return Err(AppError::Upstream(format!("yt-dlp failed: {tail}")));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| AppError::Upstream(e.to_string()))
}

pub async fn probe(cfg: &Config, video: &VideoId) -> AppResult<Metadata> {
    let meta = run_dump(cfg, video).await?;
    if meta.is_live {
        return Err(AppError::BadRequest(
            "Không xử lý video đang phát trực tiếp.".into(),
        ));
    }
    Ok(meta)
}

pub fn source_language(meta: &Metadata) -> AppResult<&'static str> {
    if let Some(lang) = meta.language.as_deref().and_then(supported_language) {
        return Ok(lang);
    }
    let mut candidates: Vec<&str> = meta
        .subtitles
        .keys()
        .chain(meta.automatic_captions.keys())
        .map(String::as_str)
        .collect();
    candidates.sort();
    for lang in candidates {
        if let Some(supported) = supported_language(lang) {
            return Ok(supported);
        }
    }
    Err(AppError::BadRequest(
        "Chỉ hỗ trợ tiếng Việt, Anh, Hàn.".into(),
    ))
}

fn score_track(language: &str, key: &str, automatic: bool) -> i32 {
    let exact = supported_language(key) == Some(language);
    let base = supported_language(key).is_some();
    let orig = key.ends_with("-orig");
    let mut score = 0;
    if exact {
        score += 100;
    }
    if base {
        score += 20;
    }
    if orig {
        score += 5;
    }
    if !automatic {
        score += 10;
    }
    score
}

pub fn choose_caption(meta: &Metadata, language: &str) -> Option<Caption> {
    let mut best: Option<(i32, Caption)> = None;
    for (key, tracks) in &meta.subtitles {
        if supported_language(key).is_none() {
            continue;
        }
        for track in tracks {
            if !matches!(track.ext.as_str(), "json3" | "vtt") {
                continue;
            }
            let caption = Caption {
                lang: key.clone(),
                ext: track.ext.clone(),
                automatic: false,
            };
            let score =
                score_track(language, key, false) + if track.ext == "json3" { 2 } else { 0 };
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, caption));
            }
        }
    }
    for (key, tracks) in &meta.automatic_captions {
        if supported_language(key).is_none() {
            continue;
        }
        for track in tracks {
            if !matches!(track.ext.as_str(), "json3" | "vtt") {
                continue;
            }
            let caption = Caption {
                lang: key.clone(),
                ext: track.ext.clone(),
                automatic: true,
            };
            let score = score_track(language, key, true) + if track.ext == "json3" { 1 } else { 0 };
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, caption));
            }
        }
    }
    best.map(|(_, caption)| caption)
}

pub async fn download_caption(
    cfg: &Config,
    video: &VideoId,
    caption: &Caption,
    dir: &Path,
    uuid: &str,
) -> AppResult<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let mut args = vec![
        "--skip-download".to_string(),
        "--no-playlist".to_string(),
        "--sub-langs".to_string(),
        caption.lang.clone(),
        "--sub-format".to_string(),
        caption.ext.clone(),
        "-o".to_string(),
        dir.join(format!("{uuid}.%(ext)s")).display().to_string(),
    ];
    if caption.automatic {
        args.push("--write-auto-subs".into());
    } else {
        args.push("--write-subs".into());
    }
    args.push(format!("https://www.youtube.com/watch?v={}", video.id));

    let output = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new(&cfg.ytdlp_path)
            .kill_on_drop(true)
            .args(&args)
            .env("HOME", cfg.data_dir.join("tmp/yt-dlp"))
            .env("XDG_CACHE_HOME", cfg.data_dir.join("tmp/yt-dlp/cache"))
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| AppError::Upstream("yt-dlp subtitle timed out".into()))?
    .map_err(|e| AppError::Internal(e.into()))?;

    if !output.status.success() {
        let tail = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Upstream(format!(
            "yt-dlp subtitle failed: {tail}"
        )));
    }
    let path = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension().and_then(|v| v.to_str()) == Some(caption.ext.as_str())
                && path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .is_some_and(|name| name.starts_with(uuid))
        })
        .ok_or_else(|| AppError::Upstream("subtitle file was not created".into()))?;
    Ok(path)
}

pub fn classify_source_error(error: &AppError) -> &'static str {
    match error {
        AppError::BadRequest(m) if m.contains("playlist") => "playlist_not_supported",
        AppError::BadRequest(m) if m.contains("trực tiếp") => "live_not_supported",
        AppError::BadRequest(m) if m.contains("tiếng Việt") => "unsupported_language",
        _ => "source_unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_codecs_and_orders_youtube_default_first() {
        let meta: Metadata =
            serde_json::from_str(include_str!("../../tests/fixtures/ytdlp_audio_tracks.json"))
                .unwrap();
        let tracks = audio_tracks(&meta);
        assert_eq!(tracks.len(), 3);
        assert!(tracks[0].is_default);
        assert_eq!(tracks[0].source_language.as_deref(), Some("en"));
        assert!(tracks
            .iter()
            .any(|track| track.is_original && track.source_language.as_deref() == Some("ko")));
        let mut changed = meta.clone();
        changed
            .formats
            .retain(|format| format.format_id != "140-en");
        assert_eq!(tracks[0].key, audio_tracks(&changed)[0].key);
    }

    #[test]
    fn suggests_a_track_when_default_flag_is_missing() {
        let meta: Metadata = serde_json::from_str(r#"{"title":"x","formats":[{"format_id":"140","acodec":"aac","vcodec":"none","format_note":"low"},{"format_id":"251","acodec":"opus","vcodec":"none","format_note":"medium"}]}"#).unwrap();
        let tracks = audio_tracks(&meta);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].format_id, "140");
        assert!(!tracks[0].is_default);
        let empty: Metadata = serde_json::from_str(r#"{"title":"x"}"#).unwrap();
        assert!(audio_tracks(&empty).is_empty());
    }

    #[test]
    fn does_not_merge_distinct_tracks_with_same_language() {
        let meta: Metadata =
            serde_json::from_str(include_str!("../../tests/fixtures/ytdlp_audio_tracks.json"))
                .unwrap();
        let tracks = audio_tracks(&meta);
        let english: Vec<_> = tracks
            .iter()
            .filter(|track| track.source_language.as_deref() == Some("en"))
            .collect();
        assert_eq!(english.len(), 2);
        assert_ne!(english[0].key, english[1].key);
    }

    #[test]
    fn groups_real_extractor_metadata_and_prefers_audio_only() {
        // Safe fields from the authorized multitrack probe; no media URLs are retained.
        let meta: Metadata = serde_json::from_str(r#"{"title":"sample","formats":[
            {"format_id":"139-0","format_note":"English (US), low","language":"en-US","language_preference":-1,"acodec":"mp4a.40.5","vcodec":"none"},
            {"format_id":"139-1","format_note":"Urdu original (default), low","language":"ur","language_preference":10,"acodec":"mp4a.40.5","vcodec":"none"},
            {"format_id":"251-1","format_note":"Urdu original (default), medium","language":"ur","language_preference":10,"acodec":"opus","vcodec":"none"}
        ]}"#).unwrap();
        let tracks = audio_tracks(&meta);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].source_language.as_deref(), Some("ur"));
        assert!(tracks[0].is_original && tracks[0].is_default);

        let meta: Metadata = serde_json::from_str(
            r#"{"title":"x","formats":[
            {"format_id":"muxed","acodec":"aac","vcodec":"h264","audio_track_id":"track.1"},
            {"format_id":"audio","acodec":"opus","vcodec":"none","audio_track_id":"track.1"}
        ]}"#,
        )
        .unwrap();
        assert_eq!(audio_tracks(&meta)[0].format_id, "audio");
    }

    #[test]
    fn accepts_watch_shorts_mobile_and_short_links() {
        assert_eq!(
            validate_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
                .unwrap()
                .id,
            "dQw4w9WgXcQ"
        );
        assert_eq!(
            validate_url("https://youtube.com/shorts/dQw4w9WgXcQ")
                .unwrap()
                .id,
            "dQw4w9WgXcQ"
        );
        assert_eq!(
            validate_url("https://m.youtube.com/watch?v=dQw4w9WgXcQ&list=PL123")
                .unwrap()
                .id,
            "dQw4w9WgXcQ"
        );
        assert_eq!(
            validate_url("https://youtu.be/dQw4w9WgXcQ").unwrap().id,
            "dQw4w9WgXcQ"
        );
    }

    #[test]
    fn rejects_playlists_channels_and_other_hosts() {
        assert!(validate_url("https://www.youtube.com/playlist?list=PL123").is_err());
        assert!(validate_url("https://www.youtube.com/@channel").is_err());
        assert!(validate_url("https://example.com/watch?v=dQw4w9WgXcQ").is_err());
        assert!(validate_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn parses_fixture_and_chooses_manual_then_auto_caption() {
        let meta: Metadata =
            serde_json::from_str(include_str!("../../tests/fixtures/ytdlp_vi.json")).unwrap();
        assert_eq!(source_language(&meta).unwrap(), "vi");
        let caption = choose_caption(&meta, "vi").unwrap();
        assert_eq!(caption.lang, "vi");
        assert!(!caption.automatic);
    }
}
