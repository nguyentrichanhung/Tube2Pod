use crate::config::Config;
use anyhow::{bail, Context};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

pub fn load_file(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    let mut values = HashMap::new();
    for (index, line) in content.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let invalid = || anyhow::anyhow!("invalid config at line {}", index + 1);
        let (key, value) = line.split_once('=').ok_or_else(invalid)?;
        let key = key.trim();
        let mut chars = key.chars();
        if !chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(invalid());
        }
        let value = value.trim();
        let value = if value.starts_with(['\'', '"']) {
            let quote = value.as_bytes()[0];
            if value.len() < 2 || value.as_bytes()[value.len() - 1] != quote {
                return Err(invalid());
            }
            &value[1..value.len() - 1]
        } else {
            value
        };
        if values.insert(key.to_string(), value.to_string()).is_some() {
            return Err(invalid());
        }
    }
    Ok(values)
}

pub fn portable_data_dir(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<PathBuf> {
    let get = |key| get(key).filter(|s| !s.trim().is_empty()).map(PathBuf::from);
    #[cfg(target_os = "windows")]
    let path = get("LOCALAPPDATA").map(|p| p.join("Tube2Pod"));
    #[cfg(target_os = "macos")]
    let path = get("HOME").map(|p| p.join("Library/Application Support/Tube2Pod"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let path = get("XDG_DATA_HOME")
        .or_else(|| get("HOME").map(|p| p.join(".local/share")))
        .map(|p| p.join("tube2pod"));
    path.context("cannot determine portable data directory")
}

pub async fn check_tools(cfg: &Config) -> anyhow::Result<()> {
    for (name, path, flag) in [
        ("yt-dlp", &cfg.ytdlp_path, "--version"),
        ("ffmpeg", &cfg.ffmpeg_path, "-version"),
        ("ffprobe", &cfg.ffprobe_path, "-version"),
    ] {
        let mut child = tokio::process::Command::new(path)
            .arg(flag)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("start {name} ({path})"))?;
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .with_context(|| format!("{name} version check timed out"))?
            .with_context(|| format!("check {name} version"))?;
        if !status.success() {
            bail!("{name} version check failed: {status}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_file_preserves_literal_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.env");
        fs::write(&path, "\u{feff}# comment\r\n\r\n TOOL = 'C:\\Media Tools\\ffmpeg.exe' \r\nCOMMAND=\"$(touch sentinel)\"\r\nVALUE=a#b=c\r\n").unwrap();
        let values = load_file(&path).unwrap();
        assert_eq!(values["TOOL"], r"C:\Media Tools\ffmpeg.exe");
        assert_eq!(values["COMMAND"], "$(touch sentinel)");
        assert_eq!(values["VALUE"], "a#b=c");
        assert!(!dir.path().join("sentinel").exists());
    }

    #[test]
    fn load_file_rejects_bad_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.env");
        for bad in [
            "A=1\nA=2",
            "OK=1\n1BAD=x",
            "OK=1\nMISSING",
            "OK=1\nBAD='secret",
        ] {
            fs::write(&path, bad).unwrap();
            let err = load_file(&path).unwrap_err().to_string();
            assert!(err.contains("line 2"), "{err}");
            assert!(!err.contains("secret"));
        }
    }
}
