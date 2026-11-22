use anyhow::{bail, Context};
use std::path::PathBuf;
use url::Url;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_admin: String,
    pub bind_public: String,
    pub bind_owner: String,
    pub public_base_url: Option<Url>,
    pub data_dir: PathBuf,
    pub ytdlp_path: String,
    pub ffmpeg_path: String,
    pub ffprobe_path: String,
}

impl Config {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let value = |key: &str, default: Option<&str>| -> Option<String> {
            get(key)
                .filter(|v| !v.trim().is_empty())
                .or_else(|| default.map(str::to_string))
        };

        let public_base_url = value("PUBLIC_BASE_URL", None)
            .map(|value| {
                let parsed = Url::parse(&value)
                    .with_context(|| format!("invalid PUBLIC_BASE_URL: {value}"))?;
                if parsed.scheme() != "https" {
                    bail!("PUBLIC_BASE_URL must use https");
                }
                if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
                    bail!("PUBLIC_BASE_URL must not contain a path, query or fragment");
                }
                Ok(parsed)
            })
            .transpose()?;

        Ok(Self {
            bind_admin: value("BIND_ADMIN", Some("127.0.0.1:8400")).unwrap(),
            bind_public: value("BIND_PUBLIC", Some("127.0.0.1:8401")).unwrap(),
            bind_owner: value("BIND_OWNER", Some("127.0.0.1:8402")).unwrap(),
            public_base_url,
            data_dir: PathBuf::from(value("DATA_DIR", Some("./data")).unwrap()),
            ytdlp_path: value("YTDLP_PATH", Some("yt-dlp")).unwrap(),
            ffmpeg_path: value("FFMPEG_PATH", Some("ffmpeg")).unwrap(),
            ffprobe_path: value("FFPROBE_PATH", Some("ffprobe")).unwrap(),
        })
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("tube2pod.db")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn trims_public_base_url_and_requires_https() {
        let cfg = Config::from_lookup(env(&[
            ("PUBLIC_BASE_URL", "https://pod.example.ts.net:8443/"),
            ("DATA_DIR", "/tmp/tube2pod-test"),
        ]))
        .unwrap();
        assert_eq!(
            cfg.public_base_url.unwrap().as_str(),
            "https://pod.example.ts.net:8443/"
        );

        let err = Config::from_lookup(env(&[(
            "PUBLIC_BASE_URL",
            "http://pod.example.ts.net:8443",
        )]))
        .unwrap_err();
        assert!(err.to_string().contains("PUBLIC_BASE_URL"));
    }

    #[test]
    fn boots_without_public_origin() {
        let cfg = Config::from_lookup(env(&[])).unwrap();
        assert!(cfg.public_base_url.is_none());
        assert_eq!(cfg.bind_owner, "127.0.0.1:8402");
    }

    #[test]
    fn defaults_use_local_tools() {
        let cfg =
            Config::from_lookup(env(&[("PUBLIC_BASE_URL", "https://pod.example.ts.net")])).unwrap();
        assert_eq!(cfg.bind_admin, "127.0.0.1:8400");
        assert_eq!(cfg.ytdlp_path, "yt-dlp");
        assert_eq!(cfg.ffmpeg_path, "ffmpeg");
        assert_eq!(cfg.ffprobe_path, "ffprobe");
    }
    #[test]
    fn accepts_configured_tool_paths_without_provider_settings() {
        let cfg = Config::from_lookup(env(&[
            ("PUBLIC_BASE_URL", "https://pod.example.ts.net"),
            ("YTDLP_PATH", "/usr/local/bin/yt-dlp"),
            ("FFMPEG_PATH", "/usr/bin/ffmpeg"),
            ("FFPROBE_PATH", "/usr/bin/ffprobe"),
        ]))
        .unwrap();
        assert_eq!(cfg.ytdlp_path, "/usr/local/bin/yt-dlp");
        assert_eq!(cfg.ffmpeg_path, "/usr/bin/ffmpeg");
        assert_eq!(cfg.ffprobe_path, "/usr/bin/ffprobe");
    }
}
