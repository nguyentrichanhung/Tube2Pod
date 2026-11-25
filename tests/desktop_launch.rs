#[cfg(unix)]
#[test]
fn desktop_opens_setup_with_existing_config() {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command, time::Duration};
    let dir = tempfile::tempdir().unwrap();
    let browser = dir.path().join(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    let opened = dir.path().join("opened");
    fs::write(
        &browser,
        format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", opened.display()),
    )
    .unwrap();
    fs::set_permissions(&browser, fs::Permissions::from_mode(0o755)).unwrap();
    let config = dir.path().join("settings with spaces.env");
    fs::write(
        &config,
        format!("DATA_DIR={}\n", dir.path().join("data space").display()),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tube2pod"))
        .args(["--desktop", "--config"])
        .arg(&config)
        .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if opened.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let url = fs::read_to_string(opened).unwrap_or_default();
    let _ = child.kill();
    let _ = child.wait();
    assert!(url.starts_with("http://localhost:"), "{url}");
}

#[cfg(unix)]
#[tokio::test]
async fn desktop_reuses_session_and_rejects_untrusted_control() {
    use std::{
        fs, net::TcpListener, os::unix::fs::PermissionsExt, process::Command, time::Duration,
    };
    #[path = "../src/test_tools.rs"]
    mod test_tools;
    let dir = tempfile::tempdir().unwrap();
    let browser = dir.path().join(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    let opened = dir.path().join("opened");
    fs::write(
        &browser,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n",
            opened.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&browser, fs::Permissions::from_mode(0o755)).unwrap();
    let tool = test_tools::tool(dir.path(), "media tool", "ok");
    let owner = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = dir.path().join("settings with spaces.env");
    fs::write(&config, format!("DATA_DIR={}\nYTDLP_PATH={}\nFFMPEG_PATH={}\nFFPROBE_PATH={}\nBIND_ADMIN=127.0.0.1:0\nBIND_PUBLIC=127.0.0.1:0\nBIND_LAN=127.0.0.1:0\nBIND_OWNER={}\n", dir.path().join("data space").display(), tool.display(), tool.display(), tool.display(), owner.local_addr().unwrap())).unwrap();
    let launch = || {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tube2pod"));
        c.args(["--desktop", "--config"])
            .arg(&config)
            .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()));
        c
    };
    let mut child = launch().spawn().unwrap();
    for _ in 0..100 {
        if opened.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let address = fs::read_to_string(&opened).unwrap();
    let (base, token) = address.trim().split_once("/#").unwrap();
    let client = reqwest::Client::new();
    let url = format!("{base}/action");
    assert_eq!(
        client
            .post(&url)
            .header("x-desktop-action", "stop")
            .header("origin", "https://evil.example")
            .header("x-desktop-token", token)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .post(&url)
            .header("x-desktop-action", "stop")
            .header("origin", base)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut last = String::new();
    for _ in 0..100 {
        last = client
            .get(format!("{base}/status"))
            .header("x-desktop-token", token)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        if last.contains("bind owner listener") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(last.contains("bind owner listener"), "{last}");
    let owner_addr = owner.local_addr().unwrap();
    drop(owner);
    assert!(launch().status().unwrap().success());
    for _ in 0..100 {
        last = client
            .get(format!("{base}/status"))
            .header("x-desktop-token", token)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        if last.contains("running") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(last.contains("running"), "{last}");
    assert!(launch().status().unwrap().success());
    for _ in 0..50 {
        if fs::read_to_string(&opened)
            .unwrap()
            .contains(&format!("http://localhost:{}", owner_addr.port()))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(fs::read_to_string(&opened)
        .unwrap()
        .contains(&format!("http://localhost:{}", owner_addr.port())));
    client
        .post(&url)
        .header("x-desktop-action", "stop")
        .header("origin", base)
        .header("x-desktop-token", token)
        .send()
        .await
        .unwrap();
    assert!(TcpListener::bind(owner_addr).is_ok());
    for action in ["stop", "install"] {
        let response = client
            .post(&url)
            .header("origin", base)
            .header("x-desktop-token", token)
            .header("x-desktop-action", action)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 409);
        assert!(TcpListener::bind(owner_addr).is_ok());
    }
    let response = client
        .post(&url)
        .header("origin", base)
        .header("x-desktop-token", token)
        .header("x-desktop-action", "start")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    for _ in 0..100 {
        last = client
            .get(format!("{base}/status"))
            .header("x-desktop-token", token)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        if last.contains("running") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(last.contains("running"), "{last}");
    client
        .post(&url)
        .header("origin", base)
        .header("x-desktop-token", token)
        .header("x-desktop-action", "stop")
        .send()
        .await
        .unwrap();
    assert!(launch().status().unwrap().success());
    for _ in 0..100 {
        last = client
            .get(format!("{base}/status"))
            .header("x-desktop-token", token)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        if last.contains("running") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(last.contains("running"), "{last}");
    client
        .post(&url)
        .header("x-desktop-action", "stop")
        .header("origin", base)
        .header("x-desktop-token", token)
        .send()
        .await
        .unwrap();
    let _ = child.kill();
    let _ = child.wait();
}
