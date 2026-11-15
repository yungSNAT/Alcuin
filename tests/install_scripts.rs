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

/// Runs the README's one-liner with the download folder pointed at `url`.
fn install(url: &str, dir: &Path, home: &Path) -> Output {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("irm {url}/install.ps1 | iex"),
        ]);
        command.env("ALCUIN_NO_MODIFY_PATH", "1");
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
