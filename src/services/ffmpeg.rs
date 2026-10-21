use crate::config::Config;
use crate::error::{AppError, AppResult};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Probe {
    pub bytes: u64,
    pub duration_s: f64,
}

pub fn concat_line(path: &str) -> String {
    format!("file '{}'", path.replace('\'', "'\\''"))
}

pub async fn concat_audio(
    cfg: &Config,
    parts: &[PathBuf],
    tmp_dir: &Path,
    out: &Path,
) -> AppResult<Probe> {
    if parts.is_empty() {
        return Err(AppError::BadRequest("no audio parts to assemble".into()));
    }
    tokio::fs::create_dir_all(tmp_dir).await?;
    let list_path = tmp_dir.join("concat.txt");
    let mut list = String::new();
    for part in parts {
        let value = part.to_string_lossy();
        list.push_str(&concat_line(&value));
        list.push('\n');
    }
    tokio::fs::write(&list_path, list).await?;

    let output = tokio::time::timeout(
        Duration::from_secs(900),
        Command::new(&cfg.ffmpeg_path)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
            ])
            .arg(&list_path)
            .args([
                "-ar",
                "44100",
                "-ac",
                "1",
                "-c:a",
                "libmp3lame",
                "-b:a",
                "128k",
                "-af",
                "loudnorm=I=-16:TP=-1.5:LRA=11",
                "-f",
                "mp3",
            ])
            .arg(out)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| AppError::Upstream("ffmpeg timed out".into()))?
    .map_err(|e| AppError::Internal(e.into()))?;

    if !output.status.success() {
        let tail = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Upstream(format!("ffmpeg failed: {tail}")));
    }
    let bytes = tokio::fs::metadata(out).await?.len();
    if bytes == 0 {
        return Err(AppError::Upstream("assembled audio is empty".into()));
    }
    let duration_s = probe(cfg, out).await?;
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err(AppError::Upstream("assembled audio is empty".into()));
    }
    Ok(Probe { bytes, duration_s })
}

pub async fn probe(cfg: &Config, path: &Path) -> AppResult<f64> {
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(&cfg.ffprobe_path)
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=nw=1:nk=1",
            ])
            .arg(path)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| AppError::Upstream("ffprobe timed out".into()))?
    .map_err(|e| AppError::Internal(e.into()))?;
    if !output.status.success() {
        return Err(AppError::Upstream("ffprobe failed".into()));
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .map_err(|_| AppError::Upstream("invalid ffprobe duration".into()))
}

pub fn format_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn escapes_single_quotes_in_concat_lines() {
        assert_eq!(concat_line("it's.mp3"), "file 'it'\\''s.mp3'");
    }

    #[test]
    fn formats_duration_for_itunes() {
        assert_eq!(format_duration(0), "00:00:00");
        assert_eq!(format_duration(61), "00:01:01");
        assert_eq!(format_duration(3661), "01:01:01");
    }

    fn test_config(dir: &Path) -> Config {
        let mut cfg = Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner@example.com".into()),
            "DATA_DIR" => Some(dir.display().to_string()),
            _ => None,
        })
        .unwrap();
        cfg.ffmpeg_path = dir.join("ffmpeg").display().to_string();
        cfg.ffprobe_path = dir.join("ffprobe").display().to_string();
        cfg
    }

    fn fake_binary(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[tokio::test]
    async fn concat_audio_writes_escaped_list_and_returns_probe() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        fake_binary(
            dir.path(),
            "ffmpeg",
            "#!/bin/sh\nout=\nfor a in \"$@\"; do out=\"$a\"; done\nprintf 'audio' > \"$out\"\n",
        );
        fake_binary(dir.path(), "ffprobe", "#!/bin/sh\nprintf '12.5\\n'\n");

        let part = dir.path().join("it's.mp3");
        tokio::fs::write(&part, b"part").await.unwrap();
        let tmp_dir = dir.path().join("list");
        let out = dir.path().join("out.mp3");

        let probe = concat_audio(&cfg, std::slice::from_ref(&part), &tmp_dir, &out)
            .await
            .unwrap();

        assert_eq!(probe.bytes, 5);
        assert_eq!(probe.duration_s, 12.5);
        let list = tokio::fs::read_to_string(tmp_dir.join("concat.txt"))
            .await
            .unwrap();
        assert!(list.contains("it'\\''s.mp3'"), "list: {list}");
        assert!(list.ends_with('\n'));
    }

    #[tokio::test]
    async fn concat_audio_rejects_empty_parts() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        let err = concat_audio(&cfg, &[], dir.path(), &dir.path().join("out.mp3"))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn concat_audio_reports_ffmpeg_failure() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        fake_binary(
            dir.path(),
            "ffmpeg",
            "#!/bin/sh\nprintf 'boom' >&2\nexit 1\n",
        );
        fake_binary(dir.path(), "ffprobe", "#!/bin/sh\nprintf '12.5\\n'\n");
        let part = dir.path().join("part.mp3");
        tokio::fs::write(&part, b"part").await.unwrap();

        let err = concat_audio(
            &cfg,
            std::slice::from_ref(&part),
            dir.path(),
            &dir.path().join("out.mp3"),
        )
        .await
        .unwrap_err();
        match err {
            AppError::Upstream(message) => assert!(message.contains("boom")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn concat_audio_rejects_empty_output_before_probing() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        fake_binary(
            dir.path(),
            "ffmpeg",
            "#!/bin/sh\nout=\nfor a in \"$@\"; do out=\"$a\"; done\n: > \"$out\"\n",
        );
        fake_binary(dir.path(), "ffprobe", "#!/bin/sh\nexit 1\n");
        let part = dir.path().join("part.mp3");
        tokio::fs::write(&part, b"part").await.unwrap();

        let err = concat_audio(
            &cfg,
            std::slice::from_ref(&part),
            dir.path(),
            &dir.path().join("out.mp3"),
        )
        .await
        .unwrap_err();
        match err {
            AppError::Upstream(message) => {
                assert!(message.contains("empty"), "message: {message}")
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn concat_audio_rejects_non_finite_duration() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        fake_binary(
            dir.path(),
            "ffmpeg",
            "#!/bin/sh\nout=\nfor a in \"$@\"; do out=\"$a\"; done\nprintf 'audio' > \"$out\"\n",
        );
        fake_binary(dir.path(), "ffprobe", "#!/bin/sh\nprintf 'inf\\n'\n");
        let part = dir.path().join("part.mp3");
        tokio::fs::write(&part, b"part").await.unwrap();

        let err = concat_audio(
            &cfg,
            std::slice::from_ref(&part),
            dir.path(),
            &dir.path().join("out.mp3"),
        )
        .await
        .unwrap_err();
        match err {
            AppError::Upstream(message) => {
                assert!(message.contains("empty"), "message: {message}")
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn concat_audio_forces_mp3_format_before_output() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        let args_path = dir.path().join("args.txt");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nout=\nfor a in \"$@\"; do out=\"$a\"; done\nprintf 'audio' > \"$out\"\n",
            args_path.display()
        );
        fake_binary(dir.path(), "ffmpeg", &script);
        fake_binary(dir.path(), "ffprobe", "#!/bin/sh\nprintf '12.5\\n'\n");
        let part = dir.path().join("part.mp3");
        tokio::fs::write(&part, b"part").await.unwrap();
        let out = dir.path().join("out.bin");

        concat_audio(&cfg, std::slice::from_ref(&part), dir.path(), &out)
            .await
            .unwrap();

        let args = tokio::fs::read_to_string(&args_path).await.unwrap();
        let args: Vec<&str> = args.lines().collect();
        let out_index = args
            .iter()
            .rposition(|arg| *arg == out.to_str().unwrap())
            .unwrap();
        assert_eq!(&args[out_index - 2..out_index], &["-f", "mp3"]);
    }

    #[tokio::test]
    async fn probe_parses_trimmed_duration() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        fake_binary(dir.path(), "ffprobe", "#!/bin/sh\nprintf '  9.25  \\n'\n");

        assert_eq!(
            probe(&cfg, &dir.path().join("audio.mp3")).await.unwrap(),
            9.25
        );
    }

    #[tokio::test]
    async fn probe_rejects_invalid_output() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        fake_binary(dir.path(), "ffprobe", "#!/bin/sh\nprintf 'nope\\n'\n");

        let err = probe(&cfg, &dir.path().join("audio.mp3"))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Upstream(_)));
    }
}
