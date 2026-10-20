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
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Metadata {
    pub id: String,
    pub title: String,
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

#[derive(Debug, Clone)]
pub struct Caption {
    pub lang: String,
    pub ext: String,
    pub url: String,
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
                url: track.url.clone(),
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
                url: track.url.clone(),
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
