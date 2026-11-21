use crate::config::Config;
use crate::error::{AppError, AppResult};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoId {
    pub id: String,
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
    pub is_live: bool,
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
    let mut tracks: Vec<(AudioTrack, bool, bool, usize)> = Vec::new();
    for (index, format) in meta.formats.iter().enumerate() {
        if format.format_id.is_empty() || matches!(format.acodec.as_deref(), None | Some("none")) {
            continue;
        }
        let note = format.format_note.as_deref().unwrap_or("");
        let label = note.split(',').next().unwrap_or("").trim();
        let mut label = label;
        while let Some(stripped) = label
            .strip_suffix(" (default)")
            .or_else(|| label.strip_suffix(" (original)"))
        {
            label = stripped.trim();
        }
        let label = if matches!(label, "tiny" | "low" | "medium" | "high" | "") {
            ""
        } else {
            label
        };
        let lower_note = note.to_ascii_lowercase();
        let is_default = lower_note.contains("(default)") || format.language_preference == Some(5);
        let is_original = lower_note.contains("original") || format.language_preference == Some(10);
        let identifiable = (is_default && format.language.is_none())
            || format
                .audio_track_id
                .as_deref()
                .is_some_and(|id| !id.is_empty())
            || !label.is_empty();
        let key = if let Some(id) = format.audio_track_id.as_deref().filter(|id| !id.is_empty()) {
            format!("id:{id}")
        } else if is_default && format.language.is_none() && label.is_empty() {
            "default".to_owned()
        } else {
            // yt-dlp exposes the display name and language, but not YouTube's track ID.
            format!(
                "metadata:{}",
                serde_json::to_string(&(format.language.as_deref(), label)).unwrap()
            )
        };
        let audio_only = format.vcodec.as_deref() == Some("none");
        if let Some((track, existing_audio_only, _, preferred_index)) =
            tracks.iter_mut().find(|(track, _, _, _)| track.key == key)
        {
            track.is_default |= is_default;
            track.is_original |= is_original;
            if audio_only || !*existing_audio_only {
                track.format_id.clone_from(&format.format_id);
                *existing_audio_only = audio_only;
                *preferred_index = index;
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
                index,
            ));
        }
    }
    // Unlabelled formats cannot be mapped to a particular track in a multi-track source.
    if tracks.len() > 1 {
        tracks.retain(|(_, _, identifiable, _)| *identifiable);
    }
    tracks.sort_by_key(|(track, _, _, index)| (!track.is_default, std::cmp::Reverse(*index)));
    tracks.into_iter().map(|(track, _, _, _)| track).collect()
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

pub async fn download_audio(
    cfg: &Config,
    video: &VideoId,
    format_id: &str,
    dir: &Path,
) -> AppResult<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let dir = tokio::fs::canonicalize(dir).await?;
    let template = dir.join("source.%(ext)s");
    let output = tokio::time::timeout(
        Duration::from_secs(900),
        Command::new(&cfg.ytdlp_path)
            .kill_on_drop(true)
            .args([
                "--no-playlist",
                "--no-simulate",
                "--fixup",
                "never",
                "-f",
                format_id,
                "-o",
            ])
            .arg(template)
            .args(["--print", "after_move:filepath"])
            .arg(format!("https://www.youtube.com/watch?v={}", video.id))
            .env("HOME", cfg.data_dir.join("tmp/yt-dlp"))
            .env("XDG_CACHE_HOME", cfg.data_dir.join("tmp/yt-dlp/cache"))
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| AppError::Upstream("yt-dlp timed out".into()))??;
    if !output.status.success() {
        return Err(AppError::Upstream(format!(
            "yt-dlp failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(4096)
                .collect::<String>()
        )));
    }
    let output_path = String::from_utf8(output.stdout)
        .map_err(|_| AppError::Upstream("invalid yt-dlp output path".into()))?;
    let mut lines = output_path.lines();
    let path = lines
        .next()
        .filter(|line| !line.is_empty())
        .ok_or_else(|| AppError::Upstream("missing yt-dlp output path".into()))?;
    if lines.next().is_some() {
        return Err(AppError::Upstream("multiple yt-dlp output paths".into()));
    }
    let path = tokio::fs::canonicalize(path).await?;
    if path.parent() != Some(dir.as_path()) || !tokio::fs::metadata(&path).await?.is_file() {
        return Err(AppError::Upstream(
            "yt-dlp output is outside the episode directory".into(),
        ));
    }
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

    #[tokio::test]
    async fn downloads_the_requested_format_into_episode_directory() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("yt-dlp");
        let args = root.path().join("args");
        std::fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nwhile [ $# -gt 0 ]; do if [ \"$1\" = '-o' ]; then shift; out=$1; fi; shift; done\nout=$(printf '%s' \"$out\" | sed 's/%(ext)s/webm/')\nprintf audio > \"$out\"\nprintf '%s\\n' \"$out\"\n", args.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut cfg = Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net".into()),
            "DATA_DIR" => Some(root.path().display().to_string()),
            _ => None,
        })
        .unwrap();
        cfg.ytdlp_path = script.display().to_string();
        let path = download_audio(
            &cfg,
            &VideoId {
                id: "dQw4w9WgXcQ".into(),
            },
            "251-1",
            &root.path().join("episode"),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"audio");
        let args = std::fs::read_to_string(args).unwrap();
        assert!(args.contains("-f\n251-1\n"));
        assert!(args.contains("--fixup\nnever\n"));
        assert!(args.contains("after_move:filepath"));
        let outside = root.path().join("outside");
        std::fs::write(&outside, b"audio").unwrap();
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", outside.display()),
        )
        .unwrap();
        assert!(download_audio(
            &cfg,
            &VideoId {
                id: "dQw4w9WgXcQ".into()
            },
            "251-1",
            &root.path().join("episode")
        )
        .await
        .is_err());
    }

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
        assert_eq!(tracks[0].format_id, "251");
        assert!(!tracks[0].is_default);
        let empty: Metadata = serde_json::from_str(r#"{"title":"x"}"#).unwrap();
        assert!(audio_tracks(&empty).is_empty());
    }

    #[test]
    fn prefers_later_extractor_formats_and_tracks_but_keeps_default_first() {
        let meta: Metadata = serde_json::from_str(r#"{"title":"x","formats":[
            {"format_id":"default-low","acodec":"aac","vcodec":"none","language":"en","format_note":"English (default), low"},
            {"format_id":"other-low","acodec":"aac","vcodec":"none","language":"ko","format_note":"Korean, low"},
            {"format_id":"default-high","acodec":"opus","vcodec":"none","language":"en","format_note":"English (default), medium"},
            {"format_id":"other-high","acodec":"opus","vcodec":"none","language":"ko","format_note":"Korean, medium"}
        ]}"#).unwrap();
        let tracks = audio_tracks(&meta);
        assert_eq!(
            tracks
                .iter()
                .map(|t| t.format_id.as_str())
                .collect::<Vec<_>>(),
            ["default-high", "other-high"]
        );
        let mut no_default = meta.clone();
        for format in &mut no_default.formats {
            format.format_note = format
                .format_note
                .as_ref()
                .map(|note| note.replace(" (default)", ""));
        }
        assert_eq!(audio_tracks(&no_default)[0].format_id, "other-high");
    }

    #[test]
    fn keeps_default_track_without_language_alongside_named_tracks() {
        let meta: Metadata = serde_json::from_str(r#"{"title":"x","formats":[
            {"format_id":"default-low","acodec":"aac","vcodec":"none","format_note":"Original (default), low"},
            {"format_id":"english","acodec":"aac","vcodec":"none","language":"en","format_note":"English, medium"},
            {"format_id":"default-high","acodec":"opus","vcodec":"none","format_note":"Original (default), medium"}
        ]}"#).unwrap();
        let tracks = audio_tracks(&meta);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].format_id, "default-high");
        assert!(tracks[0].is_default);
        assert_eq!(tracks[0].source_language, None);
        assert_eq!(tracks[1].source_language.as_deref(), Some("en"));
    }

    #[test]
    fn groups_markers_in_either_order_and_keeps_descriptive_tracks_without_language() {
        let meta: Metadata = serde_json::from_str(r#"{"title":"x","formats":[
            {"format_id":"first","acodec":"aac","vcodec":"none","language":"en","format_note":"English (default) (original), low"},
            {"format_id":"second","acodec":"opus","vcodec":"none","language":"en","format_note":"English (original) (default), medium"},
            {"format_id":"commentary","acodec":"aac","vcodec":"none","format_note":"Commentary, medium"}
        ]}"#).unwrap();
        let tracks = audio_tracks(&meta);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].format_id, "second");
        assert!(tracks[0].is_default && tracks[0].is_original);
        assert_eq!(tracks[1].format_id, "commentary");
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
}
