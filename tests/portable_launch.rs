#[path = "../src/test_tools.rs"]
mod test_tools;

use std::{fs, net::TcpListener, path::Path, process::Command};

fn config(dir: &Path, extra: &str) -> std::path::PathBuf {
    let tool = test_tools::tool(dir, "media tool", "ok");
    let path = dir.join("settings with spaces.env");
    fs::write(&path, format!("DATA_DIR={}\nYTDLP_PATH={}\nFFMPEG_PATH={}\nFFPROBE_PATH={}\nBIND_ADMIN=127.0.0.1:0\nBIND_PUBLIC=127.0.0.1:0\nBIND_OWNER=127.0.0.1:0\nBIND_LAN=127.0.0.1:0\n{extra}", dir.join("data space").display(), tool.display(), tool.display(), tool.display())).unwrap();
    path
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tube2pod"));
    command.env_clear();
    // Windows needs SystemRoot to initialize its network providers.
    #[cfg(windows)]
    command.env("SystemRoot", std::env::var_os("SystemRoot").unwrap());
    command
}

fn fails(args: &[&std::ffi::OsStr], expected: &str) {
    let output = command().args(args).output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(expected), "expected {expected}: {stderr}");
}

#[test]
fn rejects_invalid_arguments_config_and_tools() {
    let dir = tempfile::tempdir().unwrap();
    let flag = std::ffi::OsStr::new("--config");
    fails(&[std::ffi::OsStr::new("--unknown")], "unsupported");
    fails(&[flag], "requires a path");
    fails(
        &[flag, dir.path().join("missing").as_os_str()],
        "read config",
    );
    let path = dir.path().join("bad.env");
    fs::write(&path, "BAD LINE\n").unwrap();
    fails(&[flag, path.as_os_str()], "line 1");
    fs::write(
        &path,
        format!(
            "DATA_DIR={}\nYTDLP_PATH={}\n",
            dir.path().display(),
            dir.path().join("missing tool").display()
        ),
    )
    .unwrap();
    fails(&[flag, path.as_os_str()], "yt-dlp");
    let path = config(dir.path(), "");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("BIND_ADMIN=127.0.0.1:0", "BIND_ADMIN=0.0.0.0:3");
    fs::write(&path, text).unwrap();
    fails(&[flag, path.as_os_str()], "BIND_ADMIN");
}

#[test]
fn reports_occupied_port() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let path = config(dir.path(), "");
    let text = fs::read_to_string(&path).unwrap().replace(
        "BIND_ADMIN=127.0.0.1:0",
        &format!("BIND_ADMIN={}", listener.local_addr().unwrap()),
    );
    fs::write(&path, text).unwrap();
    fails(
        &[std::ffi::OsStr::new("--config"), path.as_os_str()],
        "bind admin listener",
    );
}

#[test]
fn environment_precedence_and_empty_data_override() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let path = config(dir.path(), "");
    let output = command()
        .env("DATA_DIR", "")
        .env("BIND_ADMIN", listener.local_addr().unwrap().to_string())
        .arg("--config")
        .arg(&path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bind admin listener"), "{stderr}");
    assert!(dir.path().join("data space/tube2pod.db").exists());
}

#[test]
fn rejects_unsuccessful_tool_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = config(dir.path(), "");
    test_tools::tool(dir.path(), "media tool", "fail");
    fails(
        &[std::ffi::OsStr::new("--config"), path.as_os_str()],
        "version check failed",
    );
}

#[test]
fn times_out_unresponsive_tool() {
    let dir = tempfile::tempdir().unwrap();
    let path = config(dir.path(), "");
    test_tools::tool(
        dir.path(),
        "media tool",
        &format!(
            "delayed-write\nms=30000\nstarted={}\nfinished={}",
            dir.path().join("started").display(),
            dir.path().join("finished").display()
        ),
    );
    let started = std::time::Instant::now();
    fails(
        &[std::ffi::OsStr::new("--config"), path.as_os_str()],
        "timed out",
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
}
