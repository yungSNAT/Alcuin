//! Bootstrap tests using only temporary profiles and synthetic cookie data.

use super::*;

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cookie_bootstrap_accepts_a_session_from_the_temporary_default_profile() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".config");
    let chrome_root = config.join("google-chrome");
    let default_profile = chrome_root.join("Default");
    let network_dir = default_profile.join("Network");
    std::fs::create_dir_all(&network_dir).unwrap();

    let database = network_dir.join("Cookies");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "
            CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
            INSERT INTO meta VALUES ('version', '24'), ('last_compatible_version', '24');

            CREATE TABLE cookies (
                creation_utc INTEGER NOT NULL DEFAULT 0,
                host_key TEXT NOT NULL,
                top_frame_site_key TEXT NOT NULL DEFAULT '',
                name TEXT NOT NULL,
                value TEXT NOT NULL,
                encrypted_value BLOB NOT NULL,
                path TEXT NOT NULL,
                expires_utc INTEGER NOT NULL,
                is_secure INTEGER NOT NULL DEFAULT 1,
                is_httponly INTEGER NOT NULL DEFAULT 1,
                last_access_utc INTEGER NOT NULL DEFAULT 0,
                has_expires INTEGER NOT NULL DEFAULT 1,
                is_persistent INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 1,
                samesite INTEGER NOT NULL DEFAULT -1,
                source_scheme INTEGER NOT NULL DEFAULT 2,
                source_port INTEGER NOT NULL DEFAULT 443,
                last_update_utc INTEGER NOT NULL DEFAULT 0,
                source_type INTEGER NOT NULL DEFAULT 0,
                has_cross_site_ancestor INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO cookies
                (host_key, name, value, encrypted_value, path, expires_utc)
            VALUES
                ('.chatgpt.com', '__Secure-next-auth.session-token',
                 'PUBLIC-session', X'', '/', 15746918400000000);
            ",
        )
        .unwrap();
    drop(connection);

    // Supply the same fixture at the legacy location as well.
    let legacy_database = default_profile.join("Cookies");
    std::fs::copy(&database, &legacy_database).unwrap();
    let original = std::fs::read(&database).unwrap();

    let environment = Environment::from_iter([
        ("HOME".into(), home.path().to_str().unwrap().into()),
        ("XDG_CONFIG_HOME".into(), config.to_str().unwrap().into()),
        (
            "CHROME_USER_DATA_DIR".into(),
            chrome_root.to_str().unwrap().into(),
        ),
    ]);
    let browser = json!({
        "manualLogin": false,
        "cookieSync": true,
        "chromeProfile": "Default",
        "manualLoginProfileDir": "PUBLIC-unused-profile"
    });
    let cancel = CancellationToken::new();

    let ready = tokio::time::timeout(
        Duration::from_secs(2),
        bootstrap(&browser, &environment, home.path(), &cancel),
    )
    .await
    .expect("cookie warm-up did not finish")
    .expect("cookie bootstrap failed");
    assert!(ready, "the seeded ChatGPT session should allow startup");
    assert!(!home.path().join("PUBLIC-unused-profile").exists());
    assert_eq!(std::fs::read(database).unwrap(), original);
    assert_eq!(std::fs::read(legacy_database).unwrap(), original);
}

#[tokio::test]
async fn manual_bootstrap_reports_discovery_or_profile_failure_before_launch() {
    let home = tempfile::tempdir().unwrap();
    let missing_chrome = home.path().join("PUBLIC-missing-chrome");
    let environment = Environment::from_iter([
        ("HOME".into(), home.path().to_str().unwrap().into()),
        ("PATH".into(), home.path().to_str().unwrap().into()),
        (
            "CHROME_PATH".into(),
            missing_chrome.to_str().unwrap().into(),
        ),
    ]);
    let blocker = home.path().join("PUBLIC-blocker");
    std::fs::write(&blocker, "PUBLIC-not-a-directory").unwrap();
    let relative_profile = Path::new("PUBLIC-blocker/PUBLIC-profile");
    let profile = home.path().join(relative_profile);

    // Discovery may fall back to a system installation. If it does, verify
    // that the fixture stops bootstrap at profile setup, before any launch.
    let expected = if crate::browser_detect::chrome(&environment).is_some() {
        let Err(error) = crate::browser_profile::ProfileRegistry::open(&profile) else {
            panic!("a profile under a regular file must fail to open before browser launch");
        };
        error.to_string()
    } else {
        "No Chrome installation detected; set trusted CHROME_PATH for service bootstrap".into()
    };
    let browser = json!({
        "manualLogin": true,
        "cookieSync": false,
        "manualLoginProfileDir": relative_profile
    });
    let error = bootstrap(
        &browser,
        &environment,
        home.path(),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();

    assert_eq!(error.to_string(), expected);
    assert!(!profile.exists());
    assert_eq!(
        std::fs::read_to_string(blocker).unwrap(),
        "PUBLIC-not-a-directory"
    );
}
