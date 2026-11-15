//! The one-line installers, run as the README gives them against a local copy of a release.
#![cfg(any(target_os = "linux", windows))]
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// Serves the files in `root` over HTTP, as the release download folder and the raw script do.
fn serve(root: PathBuf) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { continue };
            let mut request = vec![];
            let mut buffer = [0; 4096];
            while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                match socket.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => request.extend_from_slice(&buffer[..count]),
                }
            }
            let request = String::from_utf8_lossy(&request);
            let path = request
                .split(' ')
                .nth(1)
                .unwrap_or("/")
                .trim_start_matches('/');
            let (status, body) = match fs::read(root.join(path)) {
                Ok(body) => ("200 OK", body),
                Err(_) => ("404 Not Found", vec![]),
            };
            let _ = socket.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = socket.write_all(&body);
        }
    });
    address
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

/// A release folder holding the built binaries as the release workflow packs them, with the
/// installer beside it; `bad` gives the archive a wrong checksum.
fn release(root: &Path, bad: bool) -> PathBuf {
    let release = root.join(if bad { "bad" } else { "release" });
    fs::create_dir_all(&release).unwrap();
    let script = if cfg!(windows) {
        "install.ps1"
    } else {
        "install.sh"
    };
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(script),
        release.join(script),
    )
    .unwrap();
    let bins = [
        env!("CARGO_BIN_EXE_alcuin"),
        env!("CARGO_BIN_EXE_alcuin-mcp"),
    ];
    let archive = if cfg!(windows) {
        let name = "alcuin-x86_64-pc-windows-msvc";
        let archive = release.join(format!("{name}.zip"));
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for bin in bins {
            let file = Path::new(bin).file_name().unwrap().to_string_lossy();
            zip.start_file(
                format!("{name}/{file}"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(&fs::read(bin).unwrap()).unwrap();
        }
        zip.finish().unwrap();
        archive
    } else {
        let name = "alcuin-x86_64-unknown-linux-gnu";
        let stage = root.join("stage").join(name);
        fs::create_dir_all(&stage).unwrap();
        for bin in bins {
            fs::copy(bin, stage.join(Path::new(bin).file_name().unwrap())).unwrap();
        }
        let archive = release.join(format!("{name}.tar.gz"));
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(root.join("stage"))
            .arg(name)
            .status()
            .unwrap();
        assert!(status.success());
        archive
    };
    let digest = if bad {
        "0".repeat(64)
    } else {
        alcuin::artifacts::sha256(&fs::read(&archive).unwrap())
    };
    let file = archive.file_name().unwrap().to_string_lossy();
    fs::write(
        release.join(format!("{file}.sha256")),
        format!("{digest}  {file}\n"),
    )
    .unwrap();
    release
}

/// Runs the README's one-liner with the download folder pointed at `url`, leaving the Windows
/// user Path alone.
fn install(url: &str, dir: &Path, home: &Path) -> Output {
    install_with_path(url, dir, home, false)
}

fn install_with_path(url: &str, dir: &Path, home: &Path, modify_path: bool) -> Output {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("irm {url}/install.ps1 | iex"),
        ]);
        if !modify_path {
            command.env("ALCUIN_NO_MODIFY_PATH", "1");
        }
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", &format!("curl -fsSL {url}/install.sh | sh")]);
        command
    };
    command
        .env("ALCUIN_DOWNLOAD_URL", url)
        .env("ALCUIN_INSTALL_DIR", dir)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn the_installer_checks_the_archive_then_installs_both_binaries() {
    let root = tempfile::tempdir().unwrap();
    let url = serve(root.path().to_owned());
    release(root.path(), false);
    release(root.path(), true);
    let dir = root.path().join("bin");
    let exe = if cfg!(windows) { ".exe" } else { "" };
    let version = env!("CARGO_PKG_VERSION");
    // Twice: the second run replaces the binaries the first installed.
    for _ in 0..2 {
        let output = install(&format!("{url}/release"), &dir, root.path());
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert!(
            text(&output.stdout).starts_with(&format!(
                "Installed alcuin {version} and alcuin-mcp in {}.\n",
                dir.display()
            )),
            "{}",
            text(&output.stdout)
        );
        for (bin, built) in [
            ("alcuin", env!("CARGO_BIN_EXE_alcuin")),
            ("alcuin-mcp", env!("CARGO_BIN_EXE_alcuin-mcp")),
        ] {
            let installed = dir.join(format!("{bin}{exe}"));
            assert!(
                fs::read(&installed).unwrap() == fs::read(built).unwrap(),
                "{bin}"
            );
        }
        let output = Command::new(dir.join(format!("alcuin{exe}")))
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(text(&output.stdout), format!("{version}\n"));
    }
    // A wrong checksum installs nothing.
    let refused = root.path().join("refused");
    let output = install(&format!("{url}/bad"), &refused, root.path());
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("checksum mismatch"),
        "{}",
        text(&output.stderr)
    );
    assert!(!refused.exists());
}

#[cfg(windows)]
const ENVIRONMENT: &str = r"HKCU\Environment";

#[cfg(windows)]
fn reg(args: &[&str]) -> Output {
    Command::new("reg").args(args).output().unwrap()
}

/// The user Path's registry type and value as stored, without expanding its entries.
#[cfg(windows)]
fn user_path() -> Option<(String, String)> {
    let output = reg(&["query", ENVIRONMENT, "/v", "Path"]);
    let listing = text(&output.stdout);
    let line = listing
        .lines()
        .find(|line| line.trim_start().starts_with("Path "))?;
    let (_, typed) = line.split_once("    REG_")?;
    let (kind, value) = typed.split_once("    ").unwrap_or((typed, ""));
    Some((format!("REG_{}", kind.trim()), value.to_owned()))
}

/// The default Windows install adds its folder to the user Path once and keeps the entries already
/// there as stored. It changes this account's user Path, so it runs only on a GitHub runner, and
/// puts the old value back.
#[cfg(windows)]
#[test]
fn the_windows_installer_adds_its_folder_to_the_user_path_as_stored() {
    if std::env::var_os("GITHUB_ACTIONS").is_none() {
        return;
    }
    struct Restore(Option<(String, String)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some((kind, value)) => reg(&[
                    "add",
                    ENVIRONMENT,
                    "/v",
                    "Path",
                    "/t",
                    kind,
                    "/d",
                    value,
                    "/f",
                ]),
                None => reg(&["delete", ENVIRONMENT, "/v", "Path", "/f"]),
            };
        }
    }
    let original = user_path();
    let _restore = Restore(original.clone());
    let kept = r"%USERPROFILE%\alcuin-path-test";
    let seeded = match &original {
        Some((_, value)) if !value.is_empty() => format!("{value};{kept}"),
        _ => kept.to_owned(),
    };
    let output = reg(&[
        "add",
        ENVIRONMENT,
        "/v",
        "Path",
        "/t",
        "REG_EXPAND_SZ",
        "/d",
        &seeded,
        "/f",
    ]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let root = tempfile::tempdir().unwrap();
    let url = serve(root.path().to_owned());
    release(root.path(), false);
    let dir = root.path().join("bin");
    // Twice: the second run finds the folder on the Path and adds nothing.
    for _ in 0..2 {
        let output = install_with_path(&format!("{url}/release"), &dir, root.path(), true);
        assert!(output.status.success(), "{}", text(&output.stderr));
    }
    let (kind, value) = user_path().unwrap();
    let entries: Vec<_> = value.split(';').collect();
    assert_eq!(kind, "REG_EXPAND_SZ", "{value}");
    assert!(entries.contains(&kept), "{value}");
    assert_eq!(
        entries
            .iter()
            .filter(|entry| Path::new(entry) == dir)
            .count(),
        1,
        "{value}"
    );
}
