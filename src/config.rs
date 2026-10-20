use anyhow::{bail, Context};
use std::path::PathBuf;
use url::Url;

#[derive(Clone, Debug)]
pub struct OmnivoiceConfig {
    pub base_url: String,
    pub auth_mode: AuthMode,
    pub chunk_chars: usize,
    pub request_timeout_secs: u64,
    pub synthesis_timeout_secs: u64,
}

#[derive(Clone, Debug)]
pub enum AuthMode {
    Tailscale { user: String },
    Password { password: String },
}

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_admin: String,
    pub bind_public: String,
    pub public_base_url: String,
    pub data_dir: PathBuf,
    pub omnivoice: OmnivoiceConfig,
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
            .context("PUBLIC_BASE_URL is required")?
            .trim_end_matches('/')
            .to_string();
        let parsed = Url::parse(&public_base_url)
            .with_context(|| format!("invalid PUBLIC_BASE_URL: {public_base_url}"))?;
        if parsed.scheme() != "https" {
            bail!("PUBLIC_BASE_URL must use https");
        }
        if parsed.path() != "/" && !parsed.path().is_empty() {
            bail!("PUBLIC_BASE_URL must not contain a path");
        }

        let auth_mode = match value("OMNIVOICE_AUTH_MODE", Some("tailscale"))
            .unwrap()
            .as_str()
        {
            "tailscale" => {
                let user = value("OMNIVOICE_TAILNET_USER", None)
                    .context("OMNIVOICE_TAILNET_USER is required in tailscale mode")?;
                AuthMode::Tailscale { user }
            }
            "password" => {
                let password = value("OMNIVOICE_PASSWORD", None)
                    .context("OMNIVOICE_PASSWORD is required in password mode")?;
                AuthMode::Password { password }
            }
            other => bail!("unsupported OMNIVOICE_AUTH_MODE: {other}"),
        };

        let chunk_chars = value("CHUNK_CHARS", Some("4500"))
            .unwrap()
            .parse::<usize>()
            .context("CHUNK_CHARS must be a number")?;
        if chunk_chars == 0 || chunk_chars > 5000 {
            bail!("CHUNK_CHARS must be between 1 and 5000");
        }

        Ok(Self {
            bind_admin: value("BIND_ADMIN", Some("127.0.0.1:8400")).unwrap(),
            bind_public: value("BIND_PUBLIC", Some("127.0.0.1:8401")).unwrap(),
            public_base_url,
            data_dir: PathBuf::from(value("DATA_DIR", Some("./data")).unwrap()),
            omnivoice: OmnivoiceConfig {
                base_url: value("OMNIVOICE_BASE_URL", Some("http://127.0.0.1:8200"))
                    .unwrap()
                    .trim_end_matches('/')
                    .to_string(),
                auth_mode,
                chunk_chars,
                request_timeout_secs: value("OMNIVOICE_TIMEOUT_SECS", Some("60"))
                    .unwrap()
                    .parse()
                    .context("OMNIVOICE_TIMEOUT_SECS must be a number")?,
                synthesis_timeout_secs: value("SYNTHESIS_TIMEOUT_SECS", Some("7200"))
                    .unwrap()
                    .parse()
                    .context("SYNTHESIS_TIMEOUT_SECS must be a number")?,
            },
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
            ("OMNIVOICE_TAILNET_USER", "owner@example.com"),
            ("DATA_DIR", "/tmp/tube2pod-test"),
        ]))
        .unwrap();
        assert_eq!(cfg.public_base_url, "https://pod.example.ts.net:8443");

        let err = Config::from_lookup(env(&[
            ("PUBLIC_BASE_URL", "http://pod.example.ts.net:8443"),
            ("OMNIVOICE_TAILNET_USER", "owner@example.com"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("PUBLIC_BASE_URL"));
    }

    #[test]
    fn defaults_are_loopback_and_speech_safe() {
        let cfg = Config::from_lookup(env(&[
            ("PUBLIC_BASE_URL", "https://pod.example.ts.net:8443"),
            ("OMNIVOICE_TAILNET_USER", "owner@example.com"),
        ]))
        .unwrap();
        assert_eq!(cfg.bind_admin, "127.0.0.1:8400");
        assert_eq!(cfg.bind_public, "127.0.0.1:8401");
        assert!(cfg.omnivoice.chunk_chars <= 5000);
        assert_eq!(cfg.omnivoice.chunk_chars, 4500);
    }

    #[test]
    fn password_mode_requires_a_password() {
        let err = Config::from_lookup(env(&[
            ("PUBLIC_BASE_URL", "https://pod.example.ts.net:8443"),
            ("OMNIVOICE_AUTH_MODE", "password"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("OMNIVOICE_PASSWORD"));
    }
}
