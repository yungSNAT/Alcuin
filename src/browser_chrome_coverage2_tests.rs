#![cfg(target_os = "linux")]

use super::*;
use crate::cdp::fake::{self, Reply};
use serde_json::json;
use std::{io::ErrorKind, os::unix::fs::symlink};

const IO_TIMEOUT: Duration = Duration::from_secs(1);

fn environment(home: &Path, tmp: &Path) -> Environment {
    Environment::from([
        ("HOME".into(), home.to_string_lossy().into_owned()),
        ("TMPDIR".into(), tmp.to_string_lossy().into_owned()),
    ])
}

fn unused_endpoint() -> AttachmentEndpoint {
    AttachmentEndpoint {
        host: "127.0.0.1".into(),
        port: 1,
        browser_ws_endpoint: "ws://127.0.0.1:1/devtools/browser/PUBLIC-unused".into(),
    }
}

fn launch_options(root: &Path) -> ChromeLaunch {
    ChromeLaunch {
        executable: root.join("PUBLIC-executable"),
        cwd: root.to_owned(),
        environment: environment(root, root),
        debug_port: None,
        timeout: IO_TIMEOUT,
        keep_browser: false,
        headless: false,
        hide_window: false,
        keep_window_behind: false,
        borrow_profile_owner: false,
    }
}

fn assert_identity_refusal<T>(result: Result<T>, message: &str) {
    let error = result.err().expect("Expected a Chrome identity refusal");
    assert!(error.downcast_ref::<ChromeIdentityRefusal>().is_some());
    assert_eq!(error.to_string(), message);
}

#[tokio::test]
async fn remote_configuration_attaches_by_discovery_or_explicit_socket() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("PUBLIC-remote-profile");
    let env = environment(root.path(), root.path());
    let cancel = CancellationToken::new();
    let remote = fake::browser(json!([]), |request| {
        if request.method == "PUBLIC.probe" {
            Reply::Result(json!({"answer": "PUBLIC-remote-browser"}))
        } else {
            Reply::ok()
        }
    })
    .await;
    let expected_socket = remote.browser_ws();

    TEST_STARTUP_LOCK
        .scope(root.path().join("PUBLIC-startup.lock"), async {
            for explicit in [false, true] {
                let mut config = json!({
                    "manualLoginProfileDir": profile,
                    "remoteChrome": {"host": "127.0.0.1", "port": remote.port}
                });
                if explicit {
                    config["remoteChromeBrowserWSEndpoint"] = json!(expected_socket);
                }
                let configured =
                    ChromeConnection::configured(&config, &env, root.path(), IO_TIMEOUT, &cancel)
                        .await
                        .unwrap();

                assert_eq!(
                    configured.profile.path(),
                    fs::canonicalize(&profile).unwrap()
                );
                assert!(configured.source_profile.is_none());
                assert!(configured.notes.is_empty());
                let connection = configured.connection;
                assert!(connection.borrowed());
                assert!(!connection.owns_process());
                assert_eq!(connection.endpoint.port, remote.port);
                assert_eq!(connection.endpoint.browser_ws_endpoint, expected_socket);
                assert_eq!(
                    connection
                        .browser
                        .call("PUBLIC.probe", json!({}), IO_TIMEOUT, &cancel)
                        .await
                        .unwrap(),
                    json!({"answer": "PUBLIC-remote-browser"})
                );
            }
        })
        .await;
}

#[tokio::test]
async fn remote_configuration_reports_invalid_settings_and_missing_discovery_identity() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("PUBLIC-remote-profile");
    let env = environment(root.path(), root.path());
    let cancel = CancellationToken::new();

    for (remote, message) in [
        (json!({"port": 1}), "Remote CDP host is required"),
        (
            json!({"host": "127.0.0.1", "port": 0}),
            "Remote CDP port is required",
        ),
        (
            json!({"host": "127.0.0.1", "port": 65_536}),
            "Remote CDP port is required",
        ),
    ] {
        let config = json!({"manualLoginProfileDir": profile, "remoteChrome": remote});
        let error = ChromeConnection::configured(&config, &env, root.path(), IO_TIMEOUT, &cancel)
            .await
            .err()
            .expect("Invalid remote settings were accepted");
        assert_eq!(error.to_string(), message);
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("GET /json/version "), "{line:?}");
        loop {
            line.clear();
            assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
            if line == "\r\n" {
                break;
            }
        }
        reader
            .get_mut()
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            )
            .await
            .unwrap();
    });

    let config = json!({
        "manualLoginProfileDir": profile,
        "remoteChrome": {"host": "127.0.0.1", "port": port}
    });
    let error = ChromeConnection::configured(&config, &env, root.path(), IO_TIMEOUT, &cancel)
        .await
        .err()
        .expect("Discovery without a browser socket was accepted");
    assert!(
        format!("{error:#}").contains("Browser version has no browser WebSocket endpoint"),
        "{error:#}"
    );
    tokio::time::timeout(IO_TIMEOUT, server)
        .await
        .unwrap()
        .unwrap();

    let blocked = root.path().join("PUBLIC-not-a-profile");
    fs::write(&blocked, "PUBLIC-keep").unwrap();
    let registry_error = ProfileRegistry::open(&blocked)
        .err()
        .expect("A regular file was accepted as a profile");
    let config = json!({
        "manualLoginProfileDir": blocked,
        "remoteChrome": {"host": "127.0.0.1", "port": port}
    });
    let error = ChromeConnection::configured(&config, &env, root.path(), IO_TIMEOUT, &cancel)
        .await
        .err()
        .expect("Configuration accepted a regular file as a profile");
    assert_eq!(format!("{error:#}"), format!("{registry_error:#}"));
    assert_eq!(fs::read_to_string(blocked).unwrap(), "PUBLIC-keep");
}

#[tokio::test]
async fn failed_copy_and_cancelled_fresh_configuration_leave_no_temporary_profile() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("PUBLIC-home");
    let tmp = root.path().join("PUBLIC-tmp");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&tmp).unwrap();
    let env = environment(&home, &tmp);
    let cancel = CancellationToken::new();
    let source = root.path().join("PUBLIC-missing-source");
    let copy_error = {
        let destination = TemporaryProfile::create(&tmp).unwrap();
        crate::browser_profile_copy::copy_profile(
            &source,
            &destination,
            Some("Default"),
            std::time::Instant::now() + IO_TIMEOUT,
            &cancel,
        )
        .expect_err("Copying a missing profile succeeded")
    };
    let config = json!({"copyProfileSource": source, "chromeProfile": "Default"});
    let error = ChromeConnection::configured(&config, &env, root.path(), IO_TIMEOUT, &cancel)
        .await
        .err()
        .expect("Configuration suppressed the profile copy failure");
    assert_eq!(format!("{error:#}"), format!("{copy_error:#}"));
    assert!(fs::read_dir(&tmp).unwrap().next().is_none());

    cancel.cancel();
    let config = json!({"chromePath": root.path().join("PUBLIC-missing-executable")});
    let error = ChromeConnection::configured(&config, &env, root.path(), IO_TIMEOUT, &cancel)
        .await
        .err()
        .expect("A cancelled fresh launch succeeded");
    assert_eq!(error.to_string(), "Chrome launch cancelled before bring-up");
    assert!(fs::read_dir(&tmp).unwrap().next().is_none());
}

#[tokio::test]
async fn recorded_attachment_refuses_changed_socket_and_stale_native_identity() {
    let root = tempfile::tempdir().unwrap();
    let options = launch_options(root.path());
    fs::write(&options.executable, "PUBLIC-fixture-image").unwrap();
    let profile = ProfileRegistry::open(&root.path().join("PUBLIC-profile")).unwrap();
    let remote = fake::browser(json!([]), |_| Reply::ok()).await;
    let control = profile.path().join("DevToolsActivePort");
    fs::write(
        &control,
        format!("{}\n/devtools/browser/PUBLIC\n", remote.port),
    )
    .unwrap();
    let endpoint = read_devtools_endpoint(profile.path()).unwrap().unwrap();
    let mut identity = ProcessIdentity::current().unwrap();
    // Linux's PID limit is well below this value; no child needs to be started or stopped.
    identity.pid = i32::MAX as u32;
    let record = LaunchRecord {
        version: 1,
        process_identity: identity,
        profile_identity: NativeEntry::open(profile.path(), true).unwrap().identity,
        executable: options.executable.clone(),
        executable_identity: NativeEntry::open(&options.executable, false)
            .unwrap()
            .identity,
        headless: options.headless,
        endpoint: Some(endpoint),
        extra: serde_json::Map::new(),
    };
    let cancel = CancellationToken::new();

    fs::write(
        &control,
        format!("{}\n/devtools/browser/PUBLIC-changed\n", remote.port),
    )
    .unwrap();
    assert_identity_refusal(
        ChromeConnection::attach_recorded(&profile, &record, &options, false, &cancel).await,
        "Chrome debugging endpoint changed; refusing reuse",
    );

    fs::write(
        &control,
        format!("{}\n/devtools/browser/PUBLIC\n", remote.port),
    )
    .unwrap();
    assert_identity_refusal(
        ChromeConnection::attach_recorded(&profile, &record, &options, false, &cancel).await,
        "Chrome process identity changed during attachment",
    );

    let bringup = DirectoryLock::acquire_async(
        &profile.path().join("PUBLIC-bringup.lock"),
        IO_TIMEOUT,
        &cancel,
    )
    .await
    .unwrap();
    let admission = DirectoryLock::acquire_async(
        &root.path().join("PUBLIC-startup.lock"),
        IO_TIMEOUT,
        &cancel,
    )
    .await
    .unwrap();
    assert_identity_refusal(
        ChromeConnection::borrow_profile_owner(
            profile,
            &[101, 102],
            &options,
            false,
            bringup,
            admission,
            &cancel,
        )
        .await,
        "Several browsers occupy this profile; refusing attachment",
    );
}

#[tokio::test]
async fn health_verification_requires_native_metadata_and_rejects_wrong_pid_or_image() {
    let root = tempfile::tempdir().unwrap();
    let identity = ProcessIdentity::current().unwrap();
    let mut reported = [identity.pid + 1, identity.pid].into_iter();
    let (browser, _peer) = fake::peer(IO_TIMEOUT, move |request| {
        assert_eq!(request.method, "SystemInfo.getProcessInfo");
        Reply::Result(json!({
            "processInfo": [{"type": "browser", "id": reported.next().unwrap()}]
        }))
    })
    .await;
    let mut connection = ChromeConnection::fixture(unused_endpoint(), browser, None, None);
    let cancel = CancellationToken::new();

    assert_eq!(
        connection
            .verify_health_identity(None, IO_TIMEOUT, &cancel)
            .await
            .unwrap_err()
            .to_string(),
        "Health Chrome has no native identity"
    );
    connection.identity = Some(identity);
    assert_eq!(
        connection
            .verify_health_identity(None, IO_TIMEOUT, &cancel)
            .await
            .unwrap_err()
            .to_string(),
        "Health Chrome has no native profile"
    );
    connection.profile = Some(ProfileRegistry::open(root.path()).unwrap());
    assert!(
        !connection
            .verify_health_identity(None, IO_TIMEOUT, &cancel)
            .await
            .unwrap()
    );

    let other_image = root.path().join("PUBLIC-other-image");
    fs::write(&other_image, "PUBLIC-not-the-running-image").unwrap();
    assert!(
        !connection
            .verify_health_identity(Some(&other_image), IO_TIMEOUT, &cancel)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn browser_pid_requires_one_browser_in_a_well_formed_process_list() {
    let cancel = CancellationToken::new();
    for (reply, expected) in [
        (
            json!({"processInfo": [{"type": "renderer", "id": 42}]}),
            Ok(None),
        ),
        (
            json!({"processInfo": [
                {"type": "browser", "id": 41}, {"type": "browser", "id": 42}
            ]}),
            Ok(None),
        ),
        (
            json!({"processInfo": [
                {"type": "renderer", "id": 41}, {"type": "browser", "id": 42}
            ]}),
            Ok(Some(42)),
        ),
        (
            json!({"processInfo": [{"type": "renderer", "id": 0}]}),
            Err("Browser process list is malformed"),
        ),
        (json!({}), Err("Browser process list is missing")),
    ] {
        let (browser, _peer) = fake::peer(IO_TIMEOUT, move |request| {
            assert_eq!(request.method, "SystemInfo.getProcessInfo");
            Reply::Result(reply.clone())
        })
        .await;
        let answer = browser_pid(&browser, IO_TIMEOUT, &cancel).await;
        assert_eq!(
            answer.map_err(|error| error.to_string()),
            expected.map_err(str::to_owned)
        );
    }
}

#[tokio::test]
async fn startup_inventory_retries_and_keeps_only_named_blank_pages() {
    let mut first = true;
    let (browser, _peer) = fake::peer(IO_TIMEOUT, move |request| {
        assert_eq!(request.method, "Target.getTargets");
        if std::mem::take(&mut first) {
            return Reply::Error("PUBLIC-inventory-not-ready".into());
        }
        Reply::Result(json!({"targetInfos": [
            {"type": "page", "url": "about:blank", "targetId": "PUBLIC-b"},
            {"type": "page", "url": "about:blank", "targetId": "PUBLIC-a"},
            {"type": "page", "url": "about:blank", "targetId": "PUBLIC-a"},
            {"type": "page", "url": "about:blank", "targetId": 5},
            {"type": "page", "url": "about:srcdoc", "targetId": "PUBLIC-other"},
            {"type": "iframe", "url": "about:blank", "targetId": "PUBLIC-frame"}
        ]}))
    })
    .await;
    let pages = startup_pages(&browser, IO_TIMEOUT).await;
    assert_eq!(
        pages.into_iter().collect::<Vec<_>>(),
        ["PUBLIC-a", "PUBLIC-b"]
    );

    let (browser, _peer) = fake::peer(IO_TIMEOUT, |_| Reply::ok()).await;
    assert!(
        startup_pages(&browser, Duration::from_millis(30))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn borrowed_connection_has_no_disposal_work_and_is_taken_by_its_session() {
    let (browser, _peer) = fake::peer(IO_TIMEOUT, |request| {
        Reply::Error(format!("Unexpected command: {}", request.method))
    })
    .await;
    let endpoint = unused_endpoint();
    let mut connection = ChromeConnection::fixture(endpoint.clone(), browser, None, None);
    connection.defer_copy_retirement(IO_TIMEOUT).await.unwrap();
    connection.settle_fresh(IO_TIMEOUT).await.unwrap();
    assert!(!connection.is_copied());
    assert!(!connection.is_fresh());
    let signal = connection.signal_target();
    assert!(signal.identity.is_none());
    assert!(signal.profile.is_none());

    let session = "PUBLIC-coverage2-deferred-session";
    defer_fresh(session, connection);
    assert!(take_deferred_fresh(Some("PUBLIC-coverage2-other-session")).is_empty());
    let mut pending = take_deferred_fresh(Some(session));
    assert_eq!(pending.len(), 1);
    let (name, connection) = pending.pop().unwrap();
    assert_eq!(name, session);
    assert_eq!(connection.endpoint, endpoint);
    assert!(connection.borrowed());
    assert!(take_deferred_fresh(Some(session)).is_empty());
}

#[tokio::test]
async fn fresh_disposal_consumes_bootstrap_locks_but_preserves_profile_without_identity() {
    let root = tempfile::tempdir().unwrap();
    let mut temporary = TemporaryProfile::create(root.path()).unwrap();
    temporary.launched();
    let registry = ProfileRegistry::open(temporary.path()).unwrap();
    let sentinel = registry.path().join("PUBLIC-profile-data");
    fs::write(&sentinel, "PUBLIC-retained").unwrap();
    let (browser, _peer) = fake::peer(IO_TIMEOUT, |request| {
        Reply::Error(format!("Unexpected command: {}", request.method))
    })
    .await;
    let mut connection = ChromeConnection::fixture(unused_endpoint(), browser, None, None);
    let cancel = CancellationToken::new();
    let startup_path = root.path().join("PUBLIC-startup.lock");
    let bringup_path = registry.path().join("oracle-browser-bringup.lock");
    connection.bootstrap_admission = Some(
        DirectoryLock::acquire_async(&startup_path, IO_TIMEOUT, &cancel)
            .await
            .unwrap(),
    );
    connection.bootstrap_lock = Some(
        DirectoryLock::acquire_async(&bringup_path, IO_TIMEOUT, &cancel)
            .await
            .unwrap(),
    );
    connection.profile = Some(registry);
    connection.fresh = Some(Box::new(FreshProfile {
        profile: temporary,
        startup_targets: Default::default(),
    }));

    let signal = connection.signal_target();
    assert!(signal.identity.is_none());
    assert_eq!(
        signal.profile.as_ref().unwrap().path(),
        sentinel.parent().unwrap()
    );
    drop(signal);

    let error = connection.dispose_fresh(IO_TIMEOUT).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Fresh Chrome process identity is unavailable"
    );
    assert!(connection.bootstrap_admission.is_none());
    assert!(connection.bootstrap_lock.is_none());
    assert!(connection.is_fresh());
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "PUBLIC-retained");
    for path in [startup_path, bringup_path] {
        DirectoryLock::acquire_async(&path, IO_TIMEOUT, &cancel)
            .await
            .unwrap()
            .release()
            .unwrap();
    }
}

#[test]
fn cleanup_without_a_copy_completes_and_unqualified_copy_is_retained() {
    let mut empty = DisposableLaunch::new(None);
    assert!(!empty.attempted);
    empty.cleanup().unwrap();
    assert!(empty.attempted);

    let root = tempfile::tempdir().unwrap();
    let temporary = TemporaryProfile::create(root.path()).unwrap();
    let sentinel = temporary.path().join("PUBLIC-copy-data");
    fs::write(&sentinel, "PUBLIC-copy-retained").unwrap();
    let mut copied = DisposableLaunch::new(Some(temporary));
    // The invalid group is rejected before any native process query.
    copied.group = Some(0);
    drop(copied);
    assert_eq!(
        fs::read_to_string(sentinel).unwrap(),
        "PUBLIC-copy-retained"
    );
}

#[test]
fn unreadable_control_records_do_not_authorize_fallback_or_profile_removal() {
    let root = tempfile::tempdir().unwrap();
    let mut fresh = TemporaryProfile::create(root.path()).unwrap();
    fresh.launched();
    let profile = fresh.path().to_owned();
    let record = profile.join("oracle-chrome-native.json");
    symlink(&record, &record).unwrap();

    let expected = NativeEntry::open(&record, false).err().unwrap();
    assert_eq!(
        format!("{:#}", read_record(&record).unwrap_err()),
        format!("{expected:#}")
    );
    assert!(!dispose_failed_fresh(fresh));
    assert_eq!(fs::read_link(&record).unwrap(), record);

    let primary = profile.join("DevToolsActivePort");
    fs::create_dir(profile.join("Default")).unwrap();
    fs::write(
        profile.join("Default/DevToolsActivePort"),
        "9222\n/devtools/browser/PUBLIC-fallback\n",
    )
    .unwrap();
    symlink(&primary, &primary).unwrap();
    let expected = NativeEntry::open(&primary, false).err().unwrap();
    assert_eq!(
        format!("{:#}", read_devtools_endpoint(&profile).unwrap_err()),
        format!("{expected:#}")
    );
    fs::remove_file(primary).unwrap();
    let fallback = read_devtools_endpoint(&profile).unwrap().unwrap();
    assert_eq!(fallback.port, 9222);
    assert_eq!(
        fallback.browser_ws_endpoint,
        "ws://127.0.0.1:9222/devtools/browser/PUBLIC-fallback"
    );

    assert!(startup_stderr(&profile).is_empty());
    let log = profile.join(STARTUP_STDERR);
    fs::write(
        &log,
        "\nPUBLIC-old\n PUBLIC-one \nPUBLIC-two\n\nPUBLIC-three\nPUBLIC-four\nPUBLIC-five\n",
    )
    .unwrap();
    assert_eq!(
        startup_stderr(&log),
        "; Chrome stderr: PUBLIC-one | PUBLIC-two | PUBLIC-three | PUBLIC-four | PUBLIC-five"
    );
}

#[test]
fn synthetic_proc_inventory_distinguishes_absence_from_unreadable_entries() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("PUBLIC-profile");
    let proc_root = root.path().join("PUBLIC-proc");
    let process = proc_root.join("101");
    fs::create_dir(&profile).unwrap();
    fs::create_dir_all(&proc_root).unwrap();
    let title = format!("chromium --user-data-dir={}", profile.display());

    assert!(!title_names_profile(&process, title.as_bytes(), &profile).unwrap());
    fs::create_dir(&process).unwrap();
    assert!(!title_names_profile(&process, b"chromium --no-first-run", &profile).unwrap());
    let error = title_names_profile(&process, title.as_bytes(), &profile).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        ErrorKind::NotFound
    );

    let exe = process.join("exe");
    fs::write(&exe, "PUBLIC-not-a-link").unwrap();
    let error = profile_processes_at(&profile, &proc_root).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        ErrorKind::InvalidInput
    );
    fs::remove_file(&exe).unwrap();
    symlink(root.path().join("chromium"), exe).unwrap();
    let error = profile_processes_at(&profile, &proc_root).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        ErrorKind::NotFound
    );

    let fd = process.join("fd");
    fs::create_dir(&fd).unwrap();
    fs::write(fd.join("PUBLIC-not-a-descriptor"), "").unwrap();
    let held = profile.join("PUBLIC-open");
    symlink(&held, fd.join("3")).unwrap();
    assert_eq!(open_paths(&process).unwrap(), [(3, held)]);
    fs::write(fd.join("4"), "PUBLIC-not-a-link").unwrap();
    assert_eq!(
        open_paths(&process).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
}

#[test]
fn windows_argument_parser_preserves_literal_quotes_and_backslashes() {
    let cases: &[(&str, &[&str])] = &[
        (" \t ", &[]),
        (r#"  "PUBLIC""quote"  "#, &[r#"PUBLIC"quote"#]),
        (r#"PUBLIC\"quote"#, &[r#"PUBLIC"quote"#]),
        (r"PUBLIC\ PUBLIC-next", &[r"PUBLIC\", "PUBLIC-next"]),
        (r"PUBLIC\\", &[r"PUBLIC\\"]),
        (r#""PUBLIC space\\" """#, &[r"PUBLIC space\", ""]),
    ];
    for &(command, expected) in cases {
        let actual = windows_arguments(command).unwrap();
        assert_eq!(actual.as_slice(), expected, "{command:?}");
    }
    assert_eq!(
        windows_arguments(r#""PUBLIC-unclosed"#)
            .unwrap_err()
            .to_string(),
        "Native Chrome command has unmatched quoting; profile occupancy is unknown"
    );
}
