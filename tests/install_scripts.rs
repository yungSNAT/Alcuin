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

/// A folder for the front of the Linux installer's PATH whose `curl` drops the script's
/// `--proto =https`, as the test server speaks plain HTTP.
fn http_shim(home: &Path) -> PathBuf {
    let shim = home.join("http-shim");
    fs::create_dir_all(&shim).unwrap();
    let curl = shim.join("curl");
    fs::write(
        &curl,
        "#!/bin/sh\n[ \"$1\" = --proto ] && shift 2\nexec /usr/bin/curl \"$@\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    }
    shim
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
        command.args([
            "-c",
            &format!(
                "curl -fsSL {url}/install.sh | PATH={}:$PATH sh",
                http_shim(home).display()
            ),
        ]);
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
    // An MCP client's alcuin-mcp keeps running through 2 updates: the first moves it aside and
    // the second must not trip over the copy the running process still holds.
    let mut running = Command::new(dir.join(format!("alcuin-mcp{exe}")))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..2 {
        let output = install(&format!("{url}/release"), &dir, root.path());
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert!(
            fs::read(dir.join(format!("alcuin-mcp{exe}"))).unwrap()
                == fs::read(env!("CARGO_BIN_EXE_alcuin-mcp")).unwrap()
        );
    }
    running.kill().unwrap();
    running.wait().unwrap();
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

/// The Linux script downloads only over HTTPS, so neither a plain HTTP mirror nor a redirect to
/// one can swap the archive and its checksum together.
#[cfg(target_os = "linux")]
#[test]
fn a_plain_http_download_installs_nothing() {
    let root = tempfile::tempdir().unwrap();
    let url = serve(root.path().to_owned());
    release(root.path(), false);
    let dir = root.path().join("bin");
    // As the README gives it, without the test's HTTP shim.
    let output = Command::new("sh")
        .args(["-c", &format!("curl -fsSL {url}/release/install.sh | sh")])
        .env("ALCUIN_DOWNLOAD_URL", format!("{url}/release"))
        .env("ALCUIN_INSTALL_DIR", &dir)
        .env("HOME", root.path())
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", text(&output.stdout));
    assert!(
        text(&output.stderr).contains("could not download"),
        "{}",
        text(&output.stderr)
    );
    assert!(!dir.exists());
}

/// The Windows script refuses a plain HTTP download folder on another machine before fetching
/// anything; this machine's own, as the tests use, is allowed.
#[cfg(windows)]
#[test]
fn a_plain_http_download_installs_nothing() {
    let root = tempfile::tempdir().unwrap();
    let url = serve(root.path().to_owned());
    release(root.path(), false);
    let dir = root.path().join("bin");
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("irm {url}/release/install.ps1 | iex"),
        ])
        .env("ALCUIN_DOWNLOAD_URL", "http://mirror.example/alcuin")
        .env("ALCUIN_INSTALL_DIR", &dir)
        .env("ALCUIN_NO_MODIFY_PATH", "1")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", text(&output.stdout));
    assert!(
        text(&output.stderr).contains("http://mirror.example/alcuin is not an HTTPS address"),
        "{}",
        text(&output.stderr)
    );
    assert!(!dir.exists());
}

/// A restore that itself fails keeps the old binary under the name it gives.
#[cfg(target_os = "linux")]
#[test]
fn an_old_binary_that_cannot_be_put_back_is_kept() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let url = serve(root.path().to_owned());
    release(root.path(), false);
    let dir = root.path().join("bin");
    fs::create_dir_all(&dir).unwrap();
    for bin in ["alcuin", "alcuin-mcp"] {
        fs::write(dir.join(bin), "PUBLIC old").unwrap();
    }
    // An mv, beside the test's curl, that fails the second rename and the move of the old
    // alcuin back.
    let mv = http_shim(root.path()).join("mv");
    fs::write(
        &mv,
        "#!/bin/sh\ncase \"$*\" in\n*.alcuin-mcp.new* | *.alcuin.old*) exit 1 ;;\nesac\nexec /bin/mv \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&mv, fs::Permissions::from_mode(0o755)).unwrap();
    let output = install(&format!("{url}/release"), &dir, root.path());
    assert!(!output.status.success(), "{}", text(&output.stdout));
    let kept = dir.join(".alcuin.old");
    assert!(
        text(&output.stderr).contains(&format!("the old alcuin is in {}.", kept.display())),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(fs::read(&kept).unwrap(), b"PUBLIC old");
    assert_eq!(fs::read(dir.join("alcuin-mcp")).unwrap(), b"PUBLIC old");
}

/// An update that cannot write its second binary leaves the old pair as it was.
#[cfg(target_os = "linux")]
#[test]
fn a_failed_update_leaves_the_old_pair_in_place() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let url = serve(root.path().to_owned());
    release(root.path(), false);
    let dir = root.path().join("bin");
    fs::create_dir_all(&dir).unwrap();
    for bin in ["alcuin", "alcuin-mcp"] {
        fs::write(dir.join(bin), "PUBLIC old").unwrap();
    }
    // The second binary's staging copy cannot be written.
    let blocked = dir.join(".alcuin-mcp.new");
    fs::create_dir(&blocked).unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(blocked.join("probe"), "").is_ok() {
        return; // Running as root: nothing can be blocked this way.
    }
    let output = install(&format!("{url}/release"), &dir, root.path());
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!output.status.success(), "{}", text(&output.stdout));
    for bin in ["alcuin", "alcuin-mcp"] {
        assert!(fs::read(dir.join(bin)).unwrap() == b"PUBLIC old", "{bin}");
    }
    assert!(!dir.join(".alcuin.new").exists());
    // The second rename fails after the first binary was replaced: the old one is put back.
    fs::remove_dir(&blocked).unwrap();
    fs::remove_file(dir.join("alcuin-mcp")).unwrap();
    let held = dir.join("alcuin-mcp");
    fs::create_dir(&held).unwrap();
    fs::set_permissions(&held, fs::Permissions::from_mode(0o555)).unwrap();
    let output = install(&format!("{url}/release"), &dir, root.path());
    fs::set_permissions(&held, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!output.status.success(), "{}", text(&output.stdout));
    assert!(
        fs::read(dir.join("alcuin")).unwrap() == b"PUBLIC old",
        "{}",
        text(&output.stderr)
    );
    for leftover in [".alcuin.new", ".alcuin.old", ".alcuin-mcp.new"] {
        assert!(!dir.join(leftover).exists(), "{leftover}");
    }
}

/// Opens `path` for reading without delete sharing, as a scanner might, so it cannot be renamed.
#[cfg(windows)]
fn hold_unshared(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ
        .open(path)
        .unwrap()
}

/// An update whose second binary cannot be moved aside, after the first was replaced, puts the
/// first old one back and leaves no staging or aside copies.
#[cfg(windows)]
#[test]
fn a_failed_update_puts_the_old_binary_back() {
    let root = tempfile::tempdir().unwrap();
    let url = serve(root.path().to_owned());
    release(root.path(), false);
    let dir = root.path().join("bin");
    fs::create_dir_all(&dir).unwrap();
    for bin in ["alcuin.exe", "alcuin-mcp.exe"] {
        fs::write(dir.join(bin), "PUBLIC old").unwrap();
    }
    // A file another program holds open without delete sharing cannot be renamed.
    let lock = hold_unshared(&dir.join("alcuin-mcp.exe"));
    let output = install(&format!("{url}/release"), &dir, root.path());
    drop(lock);
    assert!(!output.status.success(), "{}", text(&output.stdout));
    for bin in ["alcuin.exe", "alcuin-mcp.exe"] {
        assert!(
            fs::read(dir.join(bin)).unwrap() == b"PUBLIC old",
            "{bin}: {}",
            text(&output.stderr)
        );
    }
    let left: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left.len(), 2, "{left:?}");
}

/// An update stopped by a signal between its 2 renames puts the first old binary back.
#[cfg(target_os = "linux")]
#[test]
fn an_interrupted_update_puts_the_old_binary_back() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let url = serve(root.path().to_owned());
    release(root.path(), false);
    let dir = root.path().join("bin");
    fs::create_dir_all(&dir).unwrap();
    for bin in ["alcuin", "alcuin-mcp"] {
        fs::write(dir.join(bin), "PUBLIC old").unwrap();
    }
    // An mv, beside the test's curl, that sends the installer TERM in place of the second rename.
    let mv = http_shim(root.path()).join("mv");
    fs::write(
        &mv,
        "#!/bin/sh\ncase \"$*\" in\n*.alcuin-mcp.new*) kill -TERM \"$PPID\"; exit 0 ;;\nesac\nexec /bin/mv \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&mv, fs::Permissions::from_mode(0o755)).unwrap();
    let output = install(&format!("{url}/release"), &dir, root.path());
    assert!(!output.status.success(), "{}", text(&output.stdout));
    for bin in ["alcuin", "alcuin-mcp"] {
        assert!(
            fs::read(dir.join(bin)).unwrap() == b"PUBLIC old",
            "{bin}: {}",
            text(&output.stderr)
        );
    }
    for leftover in [
        ".alcuin.new",
        ".alcuin.old",
        ".alcuin-mcp.new",
        ".alcuin-mcp.old",
    ] {
        assert!(!dir.join(leftover).exists(), "{leftover}");
    }
}

/// An old binary held open, so that it cannot be moved back, is copied back: the old pair is whole
/// and the held copy is kept.
#[cfg(windows)]
#[test]
fn a_held_old_binary_is_copied_back() {
    let root = tempfile::tempdir().unwrap();
    let url = serve(root.path().to_owned());
    release(root.path(), false);
    let dir = root.path().join("bin");
    fs::create_dir_all(&dir).unwrap();
    for bin in ["alcuin.exe", "alcuin-mcp.exe"] {
        fs::write(dir.join(bin), "PUBLIC old").unwrap();
    }
    // As in the test above, alcuin-mcp.exe cannot be moved aside, so the update fails after
    // replacing alcuin.exe. Copies left by earlier installs slow the step between the two, so
    // the watcher below holds the old alcuin.exe before it is put back.
    let lock = hold_unshared(&dir.join("alcuin-mcp.exe"));
    for n in 0..300 {
        fs::write(dir.join(format!("alcuin-mcp.exe.{n}.old")), "").unwrap();
    }
    let (release_hold, released) = std::sync::mpsc::channel::<()>();
    let watched = dir.clone();
    let watcher = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while std::time::Instant::now() < deadline {
            for entry in fs::read_dir(&watched).unwrap().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("alcuin.exe.") && name.ends_with(".old") {
                    let file = hold_unshared(&entry.path());
                    released.recv().unwrap();
                    drop(file);
                    return Some(name);
                }
            }
        }
        None
    });
    let output = install(&format!("{url}/release"), &dir, root.path());
    let _ = release_hold.send(());
    let old = watcher
        .join()
        .unwrap()
        .expect("the old alcuin.exe was not held");
    drop(lock);
    let said = format!("{}{}", text(&output.stdout), text(&output.stderr));
    assert!(!output.status.success(), "{said}");
    for bin in ["alcuin.exe", "alcuin-mcp.exe"] {
        assert_eq!(
            fs::read(dir.join(bin)).ok().as_deref(),
            Some(&b"PUBLIC old"[..]),
            "{bin}: {said}"
        );
    }
    assert_eq!(fs::read(dir.join(&old)).unwrap(), b"PUBLIC old");
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
    // A Path the user keeps as REG_SZ stays REG_SZ, and the user's own variables are untouched.
    let seeded = format!("{value};{kept}");
    for args in [
        [
            "add",
            ENVIRONMENT,
            "/v",
            "Path",
            "/t",
            "REG_SZ",
            "/d",
            seeded.as_str(),
            "/f",
        ],
        [
            "add",
            ENVIRONMENT,
            "/v",
            "ALCUIN_INSTALL_NOTIFY",
            "/t",
            "REG_SZ",
            "/d",
            "PUBLIC",
            "/f",
        ],
    ] {
        let output = reg(&args);
        assert!(output.status.success(), "{}", text(&output.stderr));
    }
    let other = root.path().join("other");
    let output = install_with_path(&format!("{url}/release"), &other, root.path(), true);
    let notify = reg(&["query", ENVIRONMENT, "/v", "ALCUIN_INSTALL_NOTIFY"]);
    reg(&["delete", ENVIRONMENT, "/v", "ALCUIN_INSTALL_NOTIFY", "/f"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&notify.stdout).contains("PUBLIC"),
        "{}",
        text(&notify.stdout)
    );
    let (kind, value) = user_path().unwrap();
    assert_eq!(kind, "REG_SZ", "{value}");
    assert_eq!(value, format!("{seeded};{}", other.display()));
}
