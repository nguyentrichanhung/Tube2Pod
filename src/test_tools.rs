use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

pub fn tool(dir: &Path, name: &str, scenario: &str) -> PathBuf {
    static CACHE: OnceLock<tempfile::TempDir> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let output = Command::new("rustc")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/media_tool.rs"))
            .args(["--edition=2021", "-o"])
            .arg(
                dir.path()
                    .join(format!("media-tool{}", std::env::consts::EXE_SUFFIX)),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        dir
    });
    let path = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    // Reconfigure existing fixtures without overwriting a recently running executable on Windows.
    if !path.exists() {
        std::fs::copy(
            cache
                .path()
                .join(format!("media-tool{}", std::env::consts::EXE_SUFFIX)),
            &path,
        )
        .unwrap();
    }
    std::fs::write(path.with_extension("scenario"), scenario).unwrap();
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_fixture_copies_records_arguments_and_fails() {
        let dir = tempfile::Builder::new()
            .prefix("fixture space ")
            .tempdir()
            .unwrap();
        let source = dir.path().join("source audio");
        let out = dir.path().join("output audio");
        std::fs::write(&source, b"source bytes").unwrap();
        let executable = tool(
            dir.path(),
            "media tool",
            &format!("conversion-copy\nsource={}", source.display()),
        );
        assert!(Command::new(&executable)
            .arg("-i")
            .arg(&source)
            .arg(&out)
            .status()
            .unwrap()
            .success());
        assert_eq!(std::fs::read(&out).unwrap(), b"source bytes");
        assert!(std::fs::read_to_string(executable.with_extension("args"))
            .unwrap()
            .contains(&format!("-i\n{}\n", source.display())));
        tool(dir.path(), "media tool", "fail");
        assert_eq!(Command::new(&executable).status().unwrap().code(), Some(7));
    }
}
