#![cfg(target_os = "linux")]

use super::*;
use serde_json::json;
use std::{io::ErrorKind, time::Duration};

#[tokio::test]
async fn unavailable_gnome_keyring_preserves_legacy_cookie_extraction() {
    // Exercise lookup failure only when Secret Service has no session bus to
    // contact. Do not override the process environment or query a live keyring.
    if [
        "DBUS_SESSION_BUS_ADDRESS",
        "DBUS_STARTER_ADDRESS",
        "XDG_RUNTIME_DIR",
    ]
    .into_iter()
    .any(|name| std::env::var_os(name).is_some())
    {
        return;
    }

    // SAFETY: These calls only read the process's user IDs and take no arguments.
    let (uid, effective_uid) = unsafe { (libc::getuid(), libc::geteuid()) };
    for uid in [uid, effective_uid] {
        match std::fs::symlink_metadata(format!("/run/user/{uid}/bus")) {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            _ => return,
        }
    }

    let home = tempfile::tempdir().unwrap();
    let (source, _) = synthetic_source(home.path());
    let original = std::fs::read(&source).unwrap();
    let snapshots = home.path().join("PUBLIC-snapshots");
    let config = json!({
        "cookieSync": true,
        "chromeCookiePath": source,
        "cookieNames": ["PUBLIC-session"],
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let cancel = CancellationToken::new();

    for backend in [None, Some(" gnome ")] {
        let mut env =
            Environment::from([("TMPDIR".into(), snapshots.to_string_lossy().into_owned())]);
        if let Some(backend) = backend {
            env.insert("SWEET_COOKIE_LINUX_KEYRING".into(), backend.into());
        }

        let cookies = read_native(&config, &env, home.path(), deadline, &cancel)
            .await
            .unwrap_or_else(|error| panic!("backend {backend:?}: {error:#}"));

        assert_eq!(cookies.len(), 1, "backend {backend:?}");
        assert_eq!(cookies[0]["name"], "PUBLIC-session");
        assert_eq!(cookies[0]["value"], "PUBLIC-cookie-value");
        assert_eq!(cookies[0]["domain"], ".chatgpt.com");
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert!(std::fs::read_dir(&snapshots).unwrap().next().is_none());
    }
}
