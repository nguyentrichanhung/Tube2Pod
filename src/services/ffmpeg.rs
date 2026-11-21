use crate::config::Config;
use crate::error::{AppError, AppResult};
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::process::Command;

const EXEC_RETRIES: usize = 5;

#[derive(Debug, Clone)]
pub struct Probe {
    pub bytes: u64,
    pub duration_s: f64,
}

async fn run_with_exec_retry<F>(
    build: F,
    timeout: Duration,
    timeout_message: &str,
) -> AppResult<Output>
where
    F: Fn() -> Command,
{
    let mut delay = Duration::from_millis(10);
    for attempt in 0..=EXEC_RETRIES {
        let mut command = build();
        command.kill_on_drop(true).stdin(Stdio::null());
        match tokio::time::timeout(timeout, command.output()).await {
            Err(_) => return Err(AppError::Upstream(timeout_message.into())),
            Ok(Ok(output)) => return Ok(output),
            Ok(Err(error))
                if error.kind() == ErrorKind::ExecutableFileBusy && attempt < EXEC_RETRIES =>
            {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            Ok(Err(error)) => {
                return Err(AppError::Internal(anyhow::anyhow!(
                    "failed to start command: {error}"
                )));
            }
        }
    }
    unreachable!()
}

pub async fn convert_to_mp3(cfg: &Config, source: &Path, out: &Path) -> AppResult<Probe> {
    let output = run_with_exec_retry(
        || {
            let mut command = Command::new(&cfg.ffmpeg_path);
            command
                .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                .arg(source)
                .args([
                    "-map",
                    "0:a:0",
                    "-ac",
                    "2",
                    "-c:a",
                    "libmp3lame",
                    "-b:a",
                    "128k",
                    "-f",
                    "mp3",
                ])
                .arg(out);
            command
        },
        Duration::from_secs(900),
        "ffmpeg timed out",
    )
    .await?;
    if !output.status.success() {
        return Err(AppError::Upstream(format!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
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
    let output = run_with_exec_retry(
        || {
            let mut command = Command::new(&cfg.ffprobe_path);
            command
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=duration",
                    "-of",
                    "default=nw=1:nk=1",
                ])
                .arg(path);
            command
        },
        Duration::from_secs(30),
        "ffprobe timed out",
    )
    .await?;
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
    use std::path::PathBuf;

    #[tokio::test]
    async fn converts_selected_stream_to_stereo_and_probes_output() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        let args = dir.path().join("args");
        fake_binary(dir.path(), "ffmpeg", &format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nfor a in \"$@\"; do out=\"$a\"; done\nprintf audio > \"$out\"\n", args.display()));
        fake_binary(dir.path(), "ffprobe", "#!/bin/sh\nprintf '12.5\\n'\n");
        let result = convert_to_mp3(
            &cfg,
            &dir.path().join("source"),
            &dir.path().join("out.mp3"),
        )
        .await
        .unwrap();
        assert_eq!(result.bytes, 5);
        assert_eq!(result.duration_s, 12.5);
        let args = std::fs::read_to_string(args).unwrap();
        assert!(args.contains("-map\n0:a:0\n"));
        assert!(args.contains("-ac\n2\n"));
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
        assert!(
            matches!(err, AppError::Upstream(_)),
            "unexpected error: {err:?}"
        );
    }
}
