use anyhow::{bail, Context};
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path, time::Duration};
use tokio::io::AsyncWriteExt;

#[derive(Clone, Copy)]
enum Format {
    Raw,
    Zip,
    TarXz,
}
struct Asset {
    name: &'static str,
    url: String,
    sha256: String,
    format: Format,
}

fn asset(name: &'static str, url: &str, sha256: &str, format: Format) -> Asset {
    Asset {
        name,
        url: url.into(),
        sha256: sha256.into(),
        format,
    }
}

fn assets() -> anyhow::Result<Vec<Asset>> {
    let (yt_name, yt_hash) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => (
            "yt-dlp_linux",
            "249ddb058234276bc449b9cd9492aad9f133bda5923c61a9e39983ae114ef17d",
        ),
        ("windows", "x86_64") => (
            "yt-dlp.exe",
            "622ed48023cac1daa52a3bf3a6ac38f03514de214b5f0fa069389fd0f6d17e5c",
        ),
        ("macos", "x86_64") => (
            "yt-dlp_macos",
            "57654854886d0398a798895aadf5173e2749fbb5e249b9f3728022ab376e11fb",
        ),
        _ => bail!("no verified media tools for this operating system and architecture"),
    };
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let names = if suffix.is_empty() {
        ["yt-dlp", "ffmpeg", "ffprobe"]
    } else {
        ["yt-dlp.exe", "ffmpeg.exe", "ffprobe.exe"]
    };
    let mut result = vec![asset(
        names[0],
        &format!("https://github.com/yt-dlp/yt-dlp/releases/download/2026.11.23/{yt_name}"),
        yt_hash,
        Format::Raw,
    )];
    if cfg!(target_os = "macos") {
        for (name, hash) in [
            (
                "ffmpeg",
                "9962f7b3c01af9ac89673c91df9ef998343a280ac0857c295e2d2a301820598a",
            ),
            (
                "ffprobe",
                "fe81b028c213d6387a5c6b315692b3ceddcd31eb6dae55bbcae257a862573918",
            ),
        ] {
            result.push(asset(
                name,
                &format!("https://evermeet.cx/ffmpeg/{name}-9.1.zip"),
                hash,
                Format::Zip,
            ));
        }
    } else {
        let (file, hash, format) = if cfg!(windows) {
            (
                "ffmpeg-n9.0.2-39-g7adfbe40dc-win64-gpl-9.0.zip",
                "693f3e224bc184b40524a62ca47526cfebaf2504414021b92fe75f677ad868b9",
                Format::Zip,
            )
        } else {
            (
                "ffmpeg-n9.0.2-39-g7adfbe40dc-linux64-gpl-9.0.tar.xz",
                "73e29a56974ae5a651133a04584c7483b21afe941e17dc91d79814a758b608d3",
                Format::TarXz,
            )
        };
        let url = format!("https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-31-12-59/{file}");
        result.push(asset(names[1], &url, hash, format));
        result.push(asset(names[2], &url, hash, format));
    }
    Ok(result)
}

/// Installs missing tools in `dir`; reports the name of each tool being downloaded.
pub async fn install(dir: &Path, progress: impl FnMut(&str)) -> anyhow::Result<()> {
    install_assets(dir, &assets()?, progress).await
}

async fn version(path: &Path, name: &str) -> anyhow::Result<()> {
    let flag = if name.starts_with("yt-dlp") {
        "--version"
    } else {
        "-version"
    };
    let status = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(path)
            .arg(flag)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("version check timed out")??;
    if !status.success() {
        bail!("version check failed: {status}");
    }
    Ok(())
}

async fn install_assets(
    dir: &Path,
    assets: &[Asset],
    mut progress: impl FnMut(&str),
) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(dir)
        .await
        .context("create tools directory")?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()?;
    let temporary = dir.join(format!(".download-{}", uuid::Uuid::new_v4()));
    let mut cached: Option<(&str, &str)> = None;
    let result = async {
        for item in assets {
            let destination = dir.join(item.name);
            if version(&destination, item.name).await.is_ok() {
                continue;
            }
            progress(item.name);
            let binary = dir.join(format!(
                ".tool-{}{}",
                uuid::Uuid::new_v4(),
                if cfg!(windows) { ".exe" } else { "" }
            ));
            let result = async {
                if cached != Some((&item.url, &item.sha256)) {
                    let mut response = client
                        .get(&item.url)
                        .send()
                        .await
                        .context("download tool")?
                        .error_for_status()
                        .context("download tool")?;
                    let mut file = tokio::fs::File::create(&temporary)
                        .await
                        .context("create download file")?;
                    let mut hash = Sha256::new();
                    while let Some(chunk) =
                        response.chunk().await.context("download interrupted")?
                    {
                        hash.update(&chunk);
                        file.write_all(&chunk).await.context("write download")?;
                    }
                    file.flush().await.context("finish download")?;
                    drop(file);
                    if format!("{:x}", hash.finalize()) != item.sha256 {
                        bail!("{} integrity check failed", item.name);
                    }
                    cached = Some((&item.url, &item.sha256));
                }
                extract(&temporary, &binary, item)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
                }
                version(&binary, item.name)
                    .await
                    .with_context(|| format!("check {}", item.name))?;
                tokio::fs::rename(&binary, &destination)
                    .await
                    .with_context(|| format!("install {}", item.name))?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            let _ = tokio::fs::remove_file(&binary).await;
            result.with_context(|| format!("prepare {}", item.name))?;
        }
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_file(&temporary).await;
    result
}

fn extract(source: &Path, destination: &Path, asset: &Asset) -> anyhow::Result<()> {
    let file = std::fs::File::open(source)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    match asset.format {
        Format::Raw => {
            std::io::copy(&mut std::io::BufReader::new(file), &mut output)?;
        }
        Format::Zip => {
            let mut archive = zip::ZipArchive::new(file)?;
            let mut found = false;
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i)?;
                if entry.is_file()
                    && Path::new(entry.name())
                        .file_name()
                        .is_some_and(|n| n == asset.name)
                {
                    if found {
                        bail!("duplicate binary in archive");
                    }
                    std::io::copy(&mut entry, &mut output)?;
                    found = true;
                }
            }
            if !found {
                bail!("binary missing from archive");
            }
        }
        Format::TarXz => {
            let mut archive = tar::Archive::new(xz2::read::XzDecoder::new(file));
            let mut found = false;
            for entry in archive.entries()? {
                let mut entry = entry?;
                if entry.header().entry_type().is_file()
                    && entry.path()?.file_name().is_some_and(|n| n == asset.name)
                {
                    if found {
                        bail!("duplicate binary in archive");
                    }
                    std::io::copy(&mut entry, &mut output)?;
                    found = true;
                }
            }
            if !found {
                bail!("binary missing from archive");
            }
        }
    }
    output.flush()?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn retries_missing_tools_and_preserves_completed_installations() {
        let dir = tempfile::tempdir().unwrap();
        let script = b"#!/bin/sh\nexit 0\n";
        let hash = format!("{:x}", Sha256::digest(script));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let requests = count.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0; 1024];
                let n = stream.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                requests.fetch_add(1, Ordering::SeqCst);
                let body = if request.starts_with("GET /corrupt ") {
                    &b"bad"[..]
                } else {
                    &script[..]
                };
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len() + usize::from(request.starts_with("GET /missing "))
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                stream.write_all(body).await.unwrap();
            }
        });
        let make = |name, route| Asset {
            name,
            url: format!("http://{addr}/{route}"),
            sha256: hash.clone(),
            format: Format::Raw,
        };
        let first = vec![
            make("yt-dlp", "yt"),
            make("ffmpeg", "missing"),
            make("ffprobe", "probe"),
        ];
        assert!(install_assets(dir.path(), &first, |_| {}).await.is_err());
        assert!(dir.path().join("yt-dlp").exists());
        let retry = vec![
            make("yt-dlp", "yt"),
            make("ffmpeg", "ffmpeg"),
            make("ffprobe", "probe"),
        ];
        install_assets(dir.path(), &retry, |_| {}).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 4);
        let corrupt_dir = tempfile::tempdir().unwrap();
        assert!(
            install_assets(corrupt_dir.path(), &[make("yt-dlp", "corrupt")], |_| {})
                .await
                .is_err()
        );
        assert!(!corrupt_dir.path().join("yt-dlp").exists());
        install_assets(dir.path(), &retry, |_| {}).await.unwrap();
        std::fs::remove_file(dir.path().join("ffprobe")).unwrap();
        std::fs::create_dir(dir.path().join("ffprobe")).unwrap();
        assert!(install_assets(dir.path(), &retry, |_| {}).await.is_err());
        assert!(dir.path().join("yt-dlp").is_file());
        assert!(dir.path().join("ffmpeg").is_file());
        std::fs::remove_dir(dir.path().join("ffprobe")).unwrap();
        install_assets(dir.path(), &retry, |_| {}).await.unwrap();
        server.abort();
        install_assets(dir.path(), &retry, |_| {}).await.unwrap();
    }

    #[tokio::test]
    async fn extracts_both_binaries_from_one_verified_archive() {
        use std::io::{Cursor, Write};
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for name in ["ffmpeg", "ffprobe"] {
            zip.start_file(
                format!("bin/{name}"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        }
        let data = zip.finish().unwrap().into_inner();
        let hash = format!("{:x}", Sha256::digest(&data));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/archive", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        data.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&data).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let assets = ["ffmpeg", "ffprobe"].map(|name| Asset {
            name,
            url: url.clone(),
            sha256: hash.clone(),
            format: Format::Zip,
        });
        install_assets(dir.path(), &assets, |_| {}).await.unwrap();
        server.await.unwrap();
        assert!(dir.path().join("ffmpeg").is_file());
        assert!(dir.path().join("ffprobe").is_file());
    }

    #[tokio::test]
    #[ignore = "downloads release binaries"]
    async fn real_install() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), |name| eprintln!("installing {name}"))
            .await
            .unwrap();
    }
}
