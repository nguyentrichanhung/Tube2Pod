pub mod tools;

use crate::{config::Config, launch, lock};
use anyhow::Context;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{get as route_get, post},
    Json, Router,
};
use serde::Serialize;
use std::{
    collections::HashMap, ffi::OsString, path::PathBuf, process::Stdio, sync::Arc, time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::Mutex,
};

#[derive(Clone)]
struct App {
    inner: Arc<Mutex<Inner>>,
    active: Arc<std::sync::Mutex<String>>,
    token: String,
    origin: String,
    cfg: Config,
    file: Option<PathBuf>,
    tools: PathBuf,
}
struct Inner {
    status: String,
    detail: String,
    child: Option<tokio::process::Child>,
    owner: Option<String>,
}
#[derive(Serialize)]
struct View {
    status: String,
    detail: String,
    active: String,
}

pub async fn desktop(args: Vec<OsString>) -> anyhow::Result<()> {
    let file = match args.as_slice() {
        [] => None,
        [flag, path] if flag == "--config" => Some(PathBuf::from(path)),
        _ => anyhow::bail!("usage: --desktop [--config PATH]"),
    };
    let values = file
        .as_ref()
        .map(|p| launch::load_file(p))
        .transpose()?
        .unwrap_or_default();
    let get = |key: &str| {
        std::env::var(key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| values.get(key).cloned().filter(|v| !v.trim().is_empty()))
    };
    let data = match get("DATA_DIR") {
        Some(v) => PathBuf::from(v),
        None => launch::portable_data_dir(|k| std::env::var(k).ok())?,
    };
    tokio::fs::create_dir_all(&data).await?;
    let metadata = data.join("desktop-session.json");
    let _lock = match lock::acquire(&data.join("desktop.lock")) {
        Ok(lock) => lock,
        Err(_) => {
            for _ in 0..350 {
                if let Ok(content) = tokio::fs::read(&metadata).await {
                    if let Ok(session) = serde_json::from_slice::<HashMap<String, String>>(&content)
                    {
                        if let (Some(url), Some(token)) = (session.get("url"), session.get("token"))
                        {
                            if reqwest::Client::builder()
                                .no_proxy()
                                .build()?
                                .post(format!("{url}/reopen"))
                                .header("x-desktop-token", token)
                                .send()
                                .await
                                .is_ok_and(|r| r.status().is_success())
                            {
                                return Ok(());
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            anyhow::bail!("desktop session is already running but unavailable");
        }
    };
    let tool_dir = data.join("tools");
    let cfg = Config::from_lookup(|key| {
        if key == "DATA_DIR" {
            return Some(data.to_string_lossy().into_owned());
        }
        get(key).or_else(|| match key {
            "YTDLP_PATH" => Some(
                tool_dir
                    .join(format!("yt-dlp{}", std::env::consts::EXE_SUFFIX))
                    .to_string_lossy()
                    .into_owned(),
            ),
            "FFMPEG_PATH" => Some(
                tool_dir
                    .join(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX))
                    .to_string_lossy()
                    .into_owned(),
            ),
            "FFPROBE_PATH" => Some(
                tool_dir
                    .join(format!("ffprobe{}", std::env::consts::EXE_SUFFIX))
                    .to_string_lossy()
                    .into_owned(),
            ),
            _ => None,
        })
    })?;
    let ready = launch::check_tools(&cfg).await.is_ok();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://localhost:{}", listener.local_addr()?.port());
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let app = App {
        active: Arc::new(std::sync::Mutex::new(String::new())),
        inner: Arc::new(Mutex::new(Inner {
            status: if ready { "starting" } else { "prepare" }.into(),
            detail: String::new(),
            child: None,
            owner: None,
        })),
        token: token.clone(),
        origin: origin.clone(),
        cfg,
        file,
        tools: tool_dir,
    };
    let router = Router::new()
        .route("/", route_get(page))
        .route("/status", route_get(status))
        .route("/action", post(action))
        .route("/reopen", post(reopen))
        .with_state(app.clone());
    let info = serde_json::to_vec(&HashMap::from([("url", &origin), ("token", &token)]))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&metadata)?;
        use std::io::Write;
        f.write_all(&info)?;
    }
    #[cfg(not(unix))]
    std::fs::write(&metadata, info)?;
    open_browser(&format!("{origin}/#{token}"))?;
    if ready {
        tokio::spawn(start(app));
    }
    axum::serve(listener, router).await?;
    Ok(())
}
fn authorized(app: &App, h: &HeaderMap, browser: bool) -> bool {
    h.get(header::HOST).and_then(|v| v.to_str().ok())
        == Some(app.origin.trim_start_matches("http://"))
        && h.get("x-desktop-token").and_then(|v| v.to_str().ok()) == Some(&app.token)
        && (!browser || h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) == Some(&app.origin))
}
async fn page(State(app): State<App>, h: HeaderMap) -> impl IntoResponse {
    if h.get(header::HOST).and_then(|v| v.to_str().ok())
        != Some(app.origin.trim_start_matches("http://"))
    {
        return (StatusCode::FORBIDDEN, Html("")).into_response();
    }
    Html(
        include_str!("../../templates/desktop_setup.html")
            .replace("PLATFORM_LABEL", &platform_label()),
    )
    .into_response()
}
async fn status(State(app): State<App>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&app, &h, false) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut i = app.inner.lock().await;
    if i.child
        .as_mut()
        .is_some_and(|c| c.try_wait().ok().flatten().is_some())
    {
        i.child = None;
        i.owner = None;
        i.status = "ready".into();
    }
    Json(View {
        status: i.status.clone(),
        detail: i.detail.clone(),
        active: app.active.lock().unwrap().clone(),
    })
    .into_response()
}
async fn action(State(app): State<App>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&app, &h, true) {
        return StatusCode::FORBIDDEN;
    }
    let mut i = app.inner.lock().await;
    match i.status.as_str() {
        "prepare" | "error" | "full" => {
            i.status = "loading".into();
            i.detail.clear();
            drop(i);
            tokio::spawn(install(app));
        }
        "ready" => {
            i.status = "starting".into();
            i.detail.clear();
            drop(i);
            tokio::spawn(start(app));
        }
        "running" => {
            if let Some(mut child) = i.child.take() {
                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    let _ = std::process::Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .status();
                    if tokio::time::timeout(Duration::from_secs(10), child.wait())
                        .await
                        .is_err()
                    {
                        let _ = child.kill().await;
                    }
                }
                #[cfg(not(unix))]
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            i.owner = None;
            i.status = "ready".into();
        }
        _ => (),
    }
    StatusCode::OK
}
async fn install(app: App) {
    let active = app.active.clone();
    let result = tools::install(&app.tools, |name| {
        *active.lock().unwrap() = name.into();
    })
    .await;
    let result = match result {
        Ok(()) => launch::check_tools(&app.cfg).await,
        Err(e) => Err(e),
    };
    let mut i = app.inner.lock().await;
    match result {
        Ok(()) => {
            i.status = "ready".into();
            i.detail.clear();
        }
        Err(e) => {
            i.status = if e.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| matches!(error.raw_os_error(), Some(28 | 112)))
            }) {
                "full"
            } else {
                "error"
            }
            .into();
            i.detail = format!("{e:#}");
        }
    }
}
async fn start(app: App) {
    let result: anyhow::Result<(tokio::process::Child, String)> = async {
        let mut cmd = tokio::process::Command::new(std::env::current_exe()?);
        if let Some(file) = &app.file {
            cmd.arg("--config").arg(file);
        }
        cmd.env("DATA_DIR", &app.cfg.data_dir)
            .env("YTDLP_PATH", &app.cfg.ytdlp_path)
            .env("FFMPEG_PATH", &app.cfg.ffmpeg_path)
            .env("FFPROBE_PATH", &app.cfg.ffprobe_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().context("server stdout unavailable")?;
        let mut lines = BufReader::new(stdout).lines();
        let owner = tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(line) = lines.next_line().await? {
                if let Some(url) = line.strip_prefix("Owner: ") {
                    return Ok::<_, anyhow::Error>(url.to_string());
                }
            }
            let err = if let Some(stderr) = child.stderr.take() {
                use tokio::io::AsyncReadExt;
                let mut s = String::new();
                BufReader::new(stderr).read_to_string(&mut s).await?;
                s
            } else {
                String::new()
            };
            anyhow::bail!("{err}")
        })
        .await
        .context("server startup timed out")??;
        let client = reqwest::Client::builder().no_proxy().build()?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if client.get(&owner).send().await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .context("owner did not become ready")?;
        tokio::spawn(async move { while lines.next_line().await.ok().flatten().is_some() {} });
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while lines.next_line().await.ok().flatten().is_some() {}
            });
        }
        Ok((child, owner))
    }
    .await;
    let mut i = app.inner.lock().await;
    match result {
        Ok((child, owner)) => {
            let _ = open_browser(&owner);
            i.child = Some(child);
            i.owner = Some(owner);
            i.status = "running".into();
        }
        Err(e) => {
            i.status = "ready".into();
            i.detail = format!("{e:#}");
        }
    }
}
async fn reopen(State(app): State<App>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&app, &h, false) {
        return StatusCode::FORBIDDEN;
    }
    let mut i = app.inner.lock().await;
    if i.child
        .as_mut()
        .is_some_and(|c| c.try_wait().ok().flatten().is_some())
    {
        i.child = None;
        i.owner = None;
        i.status = "ready".into();
    }
    let _ = open_browser(&format!("{}/#{}", app.origin, app.token));
    if let Some(owner) = &i.owner {
        let _ = open_browser(owner);
    } else if i.status == "ready" {
        i.status = "starting".into();
        i.detail.clear();
        drop(i);
        tokio::spawn(start(app));
    }
    StatusCode::OK
}
fn open_browser(url: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url).spawn().context("open browser")?;
    Ok(())
}

fn platform_label() -> String {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "Windows x64",
        ("linux", "x86_64") => "Ubuntu Desktop x64",
        ("macos", "aarch64") => "macOS · Apple Silicon",
        ("macos", "x86_64") => "macOS · Intel",
        _ => "Tube2Pod",
    }
    .to_string()
}
