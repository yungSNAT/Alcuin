#![cfg(target_os = "linux")]

use super::{Environment, candidates, chrome};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const SYSTEM_BROWSER_PATHS: [&str; 14] = [
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/google-chrome-beta",
    "/usr/bin/google-chrome-unstable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/usr/bin/brave-browser",
    "/usr/bin/microsoft-edge",
    "/usr/bin/microsoft-edge-stable",
    "/snap/bin/chromium",
    "/snap/bin/brave",
    "/snap/bin/brave-browser",
    "/snap/bin/microsoft-edge",
    "/opt/google/chrome/chrome",
];

#[test]
fn linux_candidates_list_system_paths_first_and_skip_empty_path_components() {
    let home = tempfile::tempdir().unwrap();
    let first = home.path().join("PUBLIC-first-bin");
    let second = home.path().join("PUBLIC-second-bin");
    let mut environment =
        Environment::from_iter([("HOME".into(), home.path().to_str().unwrap().into())]);

    let mut expected = Vec::from(SYSTEM_BROWSER_PATHS.map(PathBuf::from));
    assert_eq!(candidates(&environment), expected);

    let empty = Path::new("");
    let path = std::env::join_paths([empty, first.as_path(), empty, second.as_path(), empty])
        .unwrap()
        .into_string()
        .unwrap();
    environment.insert("PATH".into(), path);
    expected.extend(
        [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "brave-browser",
            "microsoft-edge",
            "microsoft-edge-stable",
        ]
        .into_iter()
        .flat_map(|name| [first.join(name), second.join(name)]),
    );

    assert_eq!(candidates(&environment), expected);
}

#[test]
fn linux_chrome_fallback_skips_directories_and_non_executable_files() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("PUBLIC-bin");
    fs::create_dir(&bin).unwrap();

    let mut environment = Environment::from_iter([("PATH".into(), bin.to_str().unwrap().into())]);
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_CONFIG_DIRS",
        "XDG_DATA_HOME",
        "XDG_DATA_DIRS",
    ] {
        environment.insert(key.into(), home.path().to_str().unwrap().into());
    }

    let directory = bin.join("google-chrome");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();

    let chromium = bin.join("chromium");
    let edge = bin.join("microsoft-edge-stable");
    for path in [&chromium, &edge] {
        fs::write(path, b"PUBLIC browser fixture").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    // A directory or a non-executable file on PATH is never a browser; an executable file is.
    assert!(!super::executable(&directory));
    assert!(!super::executable(&chromium));
    assert!(!super::executable(&edge));
    fs::set_permissions(&edge, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(super::executable(&edge));
    let found = chrome(&environment).unwrap();
    assert!(
        found != chromium && found != directory,
        "{}",
        found.display()
    );
}
