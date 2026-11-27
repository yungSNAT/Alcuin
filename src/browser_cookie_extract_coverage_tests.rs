#![cfg(unix)]

use super::*;

fn unpartitioned_database(path: &Path) -> Connection {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE meta(key TEXT, value TEXT);
             INSERT INTO meta VALUES('version', '24');
             CREATE TABLE cookies(
                 host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB,
                 path TEXT, expires_utc INTEGER, is_secure INTEGER,
                 is_httponly INTEGER, samesite INTEGER
             );",
        )
        .unwrap();
    connection
}

fn select_fixture(path: &Path, home: &Path) -> crate::browser_cookie_sources::CookieSource {
    crate::browser_cookie_sources::select(
        &json!({"chromeCookiePath": path}),
        &crate::config::Environment::new(),
        home,
    )
    .unwrap()
}

#[test]
fn snapshot_rejects_missing_database_and_directory_sidecars() {
    for suffix in ["", "-wal", "-shm"] {
        let home = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(home.path()).unwrap();
        let database = root.join("PUBLIC-Cookies");
        let temporary = root.join("PUBLIC-copies");
        let entry = root.join(format!("PUBLIC-Cookies{suffix}"));

        if !suffix.is_empty() {
            fs::write(&database, b"PUBLIC-database").unwrap();
            fs::create_dir(&entry).unwrap();
        }

        let Err(error) = Snapshot::create(
            &database,
            &temporary,
            Instant::now() + Duration::from_secs(4),
            &CancellationToken::new(),
        ) else {
            panic!("snapshot accepted an unavailable or unsafe entry: {entry:?}");
        };

        assert_eq!(
            error.to_string(),
            "Chrome cookie database entry is unavailable or unsafe",
            "entry: {entry:?}"
        );
        assert!(!temporary.exists());
        if suffix.is_empty() {
            assert!(!database.exists());
        } else {
            assert!(entry.is_dir());
            assert_eq!(fs::read(&database).unwrap(), b"PUBLIC-database");
        }
    }
}

#[test]
fn unpartitioned_database_preserves_expiry_and_maps_same_site_values() {
    let home = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(home.path()).unwrap();
    let database = root.join("PUBLIC-Cookies");
    let temporary = root.join("PUBLIC-copies");
    let connection = unpartitioned_database(&database);
    let expires = jiff::Timestamp::now().as_second() + 3600;
    let chrome_expiry = (expires + 11_644_473_600) * 1_000_000;
    let cases = [
        ("PUBLIC-none", 0, chrome_expiry, Some("None")),
        ("PUBLIC-strict", 2, chrome_expiry, Some("Strict")),
        ("PUBLIC-session", -1, 0, None),
    ];

    for (name, mode, expires_utc, _) in cases {
        connection
            .execute(
                "INSERT INTO cookies
                 VALUES('.chatgpt.com', ?1, 'PUBLIC-value', X'', '/', ?2, 1, 0, ?3)",
                rusqlite::params![name, expires_utc, mode],
            )
            .unwrap();
    }

    // The host filter must scan enough unrelated rows to invoke the SQLite progress handler.
    connection
        .execute_batch(
            "WITH RECURSIVE fixture_rows(n) AS (
                 VALUES(1)
                 UNION ALL
                 SELECT n + 1 FROM fixture_rows WHERE n < 512
             )
             INSERT INTO cookies
             SELECT 'public-unrelated.invalid', 'PUBLIC-ignored', 'PUBLIC-irrelevant',
                    X'', '/', 0, 0, 0, -1
             FROM fixture_rows;",
        )
        .unwrap();
    drop(connection);

    let before = fs::read(&database).unwrap();
    let selected = select_fixture(&database, &root);
    let cookies = read_database(
        &selected,
        &temporary,
        None,
        Instant::now() + Duration::from_secs(4),
        &CancellationToken::new(),
        |_, _, _| panic!("plaintext fixture unexpectedly needed decryption"),
    )
    .unwrap();

    assert_eq!(cookies.len(), cases.len());
    for (name, _, expires_utc, same_site) in cases {
        let mut expected = json!({
            "name": name,
            "value": "PUBLIC-value",
            "domain": ".chatgpt.com",
            "path": "/",
            "secure": true,
            "httpOnly": false
        });
        if expires_utc != 0 {
            expected["expires"] = json!(expires);
        }
        if let Some(same_site) = same_site {
            expected["sameSite"] = json!(same_site);
        }
        let actual = cookies
            .iter()
            .find(|cookie| cookie["name"] == name)
            .unwrap_or_else(|| panic!("missing fixture cookie {name}"));
        assert_eq!(actual, &expected);
    }

    assert_eq!(fs::read(&database).unwrap(), before);
    assert!(fs::read_dir(&temporary).unwrap().next().is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn sqlite_open_failure_is_sanitized_and_removes_the_snapshot() {
    let home = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(home.path()).unwrap();
    let database = root.join("PUBLIC-Cookies");
    drop(unpartitioned_database(&database));

    // Exceed the bundled SQLite Unix VFS path limit without exceeding Linux's
    // filesystem limits for either the entire path or an individual component.
    let component = format!("PUBLIC-{}", "x".repeat(128));
    let mut temporary = root.join("PUBLIC-copies");
    for _ in 0..5 {
        temporary.push(&component);
    }
    fs::create_dir_all(&temporary).unwrap();

    let cancel = CancellationToken::new();
    let mut probe = Snapshot::create(
        &database,
        &temporary,
        Instant::now() + Duration::from_secs(4),
        &cancel,
    )
    .unwrap();
    let Err(sqlite_error) = Connection::open_with_flags(
        &probe.path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        panic!("SQLite unexpectedly opened the long-path snapshot");
    };
    assert_eq!(
        sqlite_error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::CannotOpen)
    );
    probe.profile.cleanup().unwrap();

    let before = fs::read(&database).unwrap();
    let selected = select_fixture(&database, &root);
    let error = read_database(
        &selected,
        &temporary,
        None,
        Instant::now() + Duration::from_secs(4),
        &cancel,
        |_, _, _| panic!("an unopened database cannot yield encrypted rows"),
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Chrome cookie database extraction failed or exceeded its limits"
    );
    assert!(fs::read_dir(&temporary).unwrap().next().is_none());
    assert_eq!(fs::read(&database).unwrap(), before);
}
