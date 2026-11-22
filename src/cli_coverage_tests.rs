//! Coverage tests for the CLI entry points, option parsing and session views.

use super::*;

fn parse(args: &[&str]) -> ArgMatches {
    let mut argv = vec!["alcuin"];
    argv.extend_from_slice(args);
    parser()
        .try_get_matches_from(argv)
        .unwrap_or_else(|error| panic!("{args:?}: {error}"))
}

/// A session view that defines `--browser-tabs`, which the captured parser leaves out.
fn tabs_view(args: &[&str]) -> ArgMatches {
    let mut argv = vec!["session"];
    argv.extend_from_slice(args);
    Command::new("session")
        .arg(Arg::new("id"))
        .arg(
            Arg::new("browser-tabs")
                .long("browser-tabs")
                .action(ArgAction::SetTrue),
        )
        .try_get_matches_from(argv)
        .unwrap_or_else(|error| panic!("{args:?}: {error}"))
}

fn temp_store() -> (tempfile::TempDir, SessionStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("oracle-home"));
    (dir, store)
}

fn os_args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

#[test]
fn timeout_seconds_accepts_auto_decimals_and_durations() {
    for (raw, expected) in [
        ("auto", Some(None)),
        (" AUTO ", Some(None)),
        ("1.5", Some(Some(1.5))),
        ("10", Some(Some(10.0))),
        ("PUBLIC", None),
    ] {
        assert_eq!(timeout_seconds(raw), expected, "{raw:?}");
    }
}

#[test]
fn output_path_keeps_stdout_and_refuses_session_storage() {
    let (_home, store) = temp_store();
    let cwd = tempfile::tempdir().unwrap();
    let env = config::runtime_environment();
    for raw in ["-", "/dev/stdout"] {
        assert_eq!(
            output_path(raw, cwd.path(), &env, &store).unwrap(),
            PathBuf::from(raw)
        );
    }
    assert_eq!(
        output_path("answer.md", cwd.path(), &env, &store).unwrap(),
        cwd.path().join("answer.md")
    );
    let inside = store.root().join("answer.md");
    let error = output_path(inside.to_str().unwrap(), cwd.path(), &env, &store).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Refusing to write output inside session storage"),
        "{error:#}"
    );
}

#[test]
fn cli_stream_finish_releases_held_carriage_return_and_prints_answers() {
    let streamed = CliStream::default();
    streamed.printed.store(true, Ordering::Relaxed);
    streamed.held_cr.store(true, Ordering::Relaxed);
    streamed.finish(Some("PUBLIC answer"), true).unwrap();
    assert!(!streamed.held_cr.load(Ordering::Relaxed));

    providers::ResponseObserver::delta(&streamed, "PUBLIC more").unwrap();
    assert!(streamed.printed.load(Ordering::Relaxed));

    let fresh = CliStream::default();
    fresh.finish(Some("PUBLIC answer\n"), true).unwrap();
    assert!(!fresh.printed.load(Ordering::Relaxed));
    assert!(!fresh.held_cr.load(Ordering::Relaxed));
}

#[tokio::test]
async fn run_prints_version_and_refuses_extra_options_on_internal_entries() {
    assert_eq!(run(os_args(&["alcuin", "--version"])).await.unwrap(), 0);
    let error = run(os_args(&[
        "alcuin",
        "--exec-window-keeper",
        "{}",
        "--keeper-ready",
        "PUBLIC-ready",
        "--model",
        "PUBLIC-model",
    ]))
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("internal worker entry"),
        "{error:#}"
    );
}

#[test]
fn internal_entry_accepts_only_its_own_option() {
    let alone = parse(&["--exec-session", "public-id"]);
    assert!(internal_entry(&alone, "exec-session", &[]).is_ok());
    let extra = parse(&["--exec-session", "public-id", "--model", "PUBLIC-model"]);
    assert_eq!(
        internal_entry(&extra, "exec-session", &[])
            .unwrap_err()
            .to_string(),
        "--exec-session is an internal worker entry and takes no other options or commands."
    );
}

#[tokio::test]
async fn subcommand_refuses_unknown_names_and_runs_docs_check() {
    let (_home, store) = temp_store();
    let cwd = tempfile::tempdir().unwrap();
    let root = parse(&[]);
    let error = subcommand(
        store,
        &root,
        "PUBLIC-unknown",
        &root,
        config::runtime_environment(),
        cwd.path().to_path_buf(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "Unknown command PUBLIC-unknown.");

    let (_home, store) = temp_store();
    let cwd_path = cwd.path().to_path_buf();
    let expected = crate::docs_check::check(&cwd_path, &Vec::<String>::new());
    let m = parse(&["docs", "check"]);
    let sub = m.subcommand_matches("docs").unwrap();
    let actual = subcommand(
        store,
        &m,
        "docs",
        sub,
        config::runtime_environment(),
        cwd_path,
    )
    .await;
    match expected {
        Ok(result) => assert_eq!(actual.unwrap(), i32::from(!result.issues.is_empty())),
        Err(error) => assert_eq!(actual.unwrap_err().to_string(), error.to_string()),
    }
}

#[tokio::test]
async fn serve_validates_run_limits_before_binding() {
    for (args, message) in [
        (
            vec!["--max-concurrent-runs", "PUBLIC"],
            "--max-concurrent-runs must be an integer",
        ),
        (
            vec!["--max-concurrent-runs", "1.5"],
            "--max-concurrent-runs must be a nonnegative safe integer",
        ),
        (
            vec!["--max-concurrent-runs=-1"],
            "--max-concurrent-runs must be a nonnegative safe integer",
        ),
        (
            vec!["--max-concurrent-runs", "0"],
            "--max-concurrent-runs must be a positive integer.",
        ),
        (
            vec!["--max-queued-runs", "2"],
            "--max-queued-runs requires --max-concurrent-runs.",
        ),
    ] {
        let (_home, store) = temp_store();
        let cwd = tempfile::tempdir().unwrap();
        let mut argv = vec!["serve"];
        argv.extend(args.iter().copied());
        let m = parse(&argv);
        let sub = m.subcommand_matches("serve").unwrap();
        let error = serve_command(
            store,
            &m,
            sub,
            config::runtime_environment(),
            cwd.path().to_path_buf(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), message, "{args:?}");
    }
}

#[tokio::test]
async fn restart_refuses_wait_with_no_wait() {
    let (_home, store) = temp_store();
    let cwd = tempfile::tempdir().unwrap();
    let env = config::runtime_environment();
    let m = parse(&["restart", "public-id", "--wait", "--no-wait"]);
    let sub = m.subcommand_matches("restart").unwrap();
    let error = restart_command(&store, &m, sub, &env, cwd.path())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Cannot combine --wait and --no-wait.");
}

#[tokio::test]
async fn project_sources_refuses_invalid_file_size_limits() {
    for (raw, message) in [
        ("PUBLIC", "max-file-size-bytes must be a positive integer"),
        ("0", "max-file-size-bytes must be positive"),
    ] {
        let (_home, store) = temp_store();
        let cwd = tempfile::tempdir().unwrap();
        let m = parse(&[
            "--browser-inline-cookies",
            "PUBLIC-inline-cookie",
            "--max-file-size-bytes",
            raw,
            "project-sources",
            "list",
        ]);
        let sub = m.subcommand_matches("project-sources").unwrap();
        let error = project_sources_command(
            store,
            &m,
            sub,
            config::runtime_environment(),
            cwd.path().to_path_buf(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), message, "{raw:?}");
    }
}

#[test]
fn file_inputs_drop_many_duplicates() {
    let cwd = tempfile::tempdir().unwrap();
    let mut argv = vec![];
    for _ in 0..10 {
        argv.extend(["--file", "PUBLIC-a.txt"]);
    }
    let m = parse(&argv);
    assert_eq!(file_inputs(&m, cwd.path()), vec!["PUBLIC-a.txt".to_owned()]);
}

#[test]
fn root_input_maps_mode_azure_inline_and_zombie_flags_and_refuses_conflicts() {
    let (_home, store) = temp_store();
    let cwd = tempfile::tempdir().unwrap();
    let env = config::runtime_environment();
    let m = parse(&[
        "--mode",
        "browser",
        "--no-azure",
        "--browser-inline-files",
        "--zombie-last-activity",
    ]);
    let (input, followup) = root_input(
        &store,
        &m,
        &env,
        cwd.path(),
        Some("PUBLIC prompt".into()),
        vec![],
        true,
    )
    .unwrap();
    assert!(followup.is_none());
    assert_eq!(input["prompt"], "PUBLIC prompt");
    assert_eq!(input["engine"], "browser");
    assert_eq!(input["provider"], "openai");
    assert_eq!(input["browserAttachments"], "never");
    assert_eq!(input["zombieUseLastActivity"], true);
    assert_eq!(input["renderOnly"], true);

    for (args, message) in [
        (
            vec!["--search", "maybe"],
            "Search mode must be \"on\" or \"off\".",
        ),
        (
            vec!["--wait", "--no-wait"],
            "Cannot combine --wait and --no-wait.",
        ),
    ] {
        let m = parse(&args);
        let error = root_input(&store, &m, &env, cwd.path(), None, vec![], false).unwrap_err();
        assert_eq!(error.to_string(), message, "{args:?}");
    }
}

#[test]
fn runtime_overrides_check_profiles_lock_timeouts_tab_caps_and_attach_conflicts() {
    let env = config::runtime_environment();

    let work = parse(&["--browser-chrome-profile", "Work"]);
    assert_eq!(
        browser_runtime_overrides_at_scope(&work, &[], &json!({}), &env)
            .unwrap_err()
            .to_string(),
        "Custom Chrome profile selection requires --copy-profile or Chrome cookie sync"
    );
    let synced =
        browser_runtime_overrides_at_scope(&work, &[], &json!({"cookieSync": true}), &env).unwrap();
    assert_eq!(synced["chromeProfile"], "Work");
    let default = parse(&["--browser-chrome-profile", "Default"]);
    assert_eq!(
        browser_runtime_overrides_at_scope(&default, &[], &json!({}), &env).unwrap()["chromeProfile"],
        "Default"
    );

    let lock = parse(&["--browser-profile-lock-timeout", "PUBLIC-bogus"]);
    assert_eq!(
        browser_runtime_overrides_at_scope(&lock, &[], &json!({}), &env).unwrap()["profileLockTimeoutMs"],
        0
    );

    let huge = parse(&["--browser-max-concurrent-tabs", "99999999999999999999999"]);
    let overrides = browser_runtime_overrides_at_scope(&huge, &[], &json!({}), &env).unwrap();
    let tabs = &overrides["maxConcurrentTabs"];
    assert!(tabs.as_u64().is_none(), "{tabs}");
    assert!((tabs.as_f64().unwrap() - 1e23).abs() < 1e10, "{tabs}");

    let plain = parse(&[]);
    let error = browser_runtime_overrides_at_scope(
        &plain,
        &[],
        &json!({"attachRunning": true, "copyProfileSource": "PUBLIC-source"}),
        &env,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("--browser-attach-running cannot be combined with --copy-profile"),
        "{error:#}"
    );
}

#[test]
fn no_routing_arguments_skips_trace_path_values() {
    for (args, expected) in [
        (
            vec!["alcuin", "--perf-trace-path", "PUBLIC-trace.json"],
            true,
        ),
        (
            vec![
                "alcuin",
                "--perf-trace",
                "--perf-trace-path",
                "PUBLIC-trace.json",
            ],
            true,
        ),
        (
            vec![
                "alcuin",
                "--perf-trace-path",
                "PUBLIC-trace.json",
                "PUBLIC prompt",
            ],
            false,
        ),
    ] {
        assert_eq!(no_routing_arguments(&os_args(&args)), expected, "{args:?}");
    }
}

#[test]
fn sources_cookie_options_map_sync_path_wait_and_error_flags() {
    let env = config::runtime_environment();
    let m = parse(&[
        "--browser-no-cookie-sync",
        "--browser-cookie-path",
        "PUBLIC-cookies.db",
        "--browser-cookie-wait",
        "PUBLIC-bogus",
        "--browser-allow-cookie-errors",
    ]);
    let mut input = json!({});
    let mut overrides = json!({});
    sources_cookie_options(&m, &[], &mut input, &mut overrides, &env);
    assert_eq!(overrides["cookieSync"], false);
    assert_eq!(overrides["manualLoginCookieSync"], false);
    assert_eq!(overrides["chromeCookiePath"], "PUBLIC-cookies.db");
    assert_eq!(overrides["cookieSyncWaitMs"], 0);
    assert_eq!(overrides["allowCookieErrors"], true);
    assert_eq!(input["browserNoCookieSync"], true);

    let m = parse(&["--browser-cookie-sync", "--browser-cookie-wait", "2s"]);
    let mut input = json!({});
    let mut overrides = json!({});
    sources_cookie_options(&m, &[], &mut input, &mut overrides, &env);
    assert_eq!(overrides["cookieSync"], true);
    assert_eq!(input["browserCookieSync"], true);
    assert_eq!(overrides["cookieSyncWaitMs"], 2000);
    assert!(overrides.get("allowCookieErrors").is_none());
}

#[tokio::test]
async fn session_command_refuses_conflicting_views() {
    let (_home, store) = temp_store();
    let env = config::runtime_environment();
    for (args, message) in [
        (
            vec!["session", "public-id", "--clear"],
            "Cannot combine a session ID with --clear. Remove the ID to delete cached sessions.",
        ),
        (
            vec!["session", "clear"],
            "Session cleanup now uses --clear. Run \"alcuin session --clear --hours <n>\" instead.",
        ),
        (
            vec!["session", "public-id", "--harvest", "--live"],
            "Cannot combine --harvest and --live. Choose one.",
        ),
    ] {
        let root = parse(&args);
        let view = root.subcommand_matches("session").unwrap();
        let error = session_command(&store, &root, view, "session", &env, &[])
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), message, "{args:?}");
    }

    let root = parse(&[]);
    let view = tabs_view(&["public-id", "--browser-tabs"]);
    let error = session_command(&store, &root, &view, "session", &env, &[])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cannot combine a session ID with --browser-tabs. Remove the ID to inspect live browser tabs."
    );
}

#[tokio::test]
async fn session_browser_tabs_reports_like_tabs_status() {
    let (_home, store) = temp_store();
    let env = config::runtime_environment();
    let expected = crate::browser_sessions::tabs_status(&store, &CancellationToken::new()).await;
    let root = parse(&[]);
    let view = tabs_view(&["--browser-tabs"]);
    let actual = tokio::time::timeout(
        Duration::from_secs(4),
        session_command(&store, &root, &view, "session", &env, &[]),
    )
    .await
    .expect("browser tab status finished");
    match expected {
        Ok(_) => assert_eq!(actual.unwrap(), 0),
        Err(error) => assert_eq!(actual.unwrap_err().to_string(), error.to_string()),
    }
}

#[tokio::test]
async fn session_path_reports_missing_files_then_succeeds() {
    let (_home, store) = temp_store();
    let env = config::runtime_environment();
    let id = "public-path-session";
    let dir = store.directory(id).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("meta.json"), json!({"id": id}).to_string()).unwrap();
    let root = parse(&["session", id, "--path"]);
    let view = root.subcommand_matches("session").unwrap();
    let error = session_command(&store, &root, view, "session", &env, &[])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Session \"{id}\" is missing: output.log")
    );
    std::fs::write(dir.join("output.log"), "PUBLIC log\n").unwrap();
    assert_eq!(
        session_command(&store, &root, view, "session", &env, &[])
            .await
            .unwrap(),
        0
    );
}

#[test]
fn print_inspected_answer_prints_and_refuses_after_cancellation() {
    let (_home, store) = temp_store();
    let id = "public-inspected";
    let root = parse(&["session", id, "--harvest"]);
    let view = root.subcommand_matches("session").unwrap();
    let cancel = CancellationToken::new();
    print_inspected_answer(&store, id, &root, view, "PUBLIC answer", &cancel).unwrap();
    print_inspected_answer(&store, id, &root, view, "", &cancel).unwrap();
    cancel.cancel();
    assert_eq!(
        print_inspected_answer(&store, id, &root, view, "PUBLIC answer", &cancel)
            .unwrap_err()
            .to_string(),
        "Browser inspection cancelled"
    );
}

#[test]
fn print_inspected_answer_writes_requested_output_file() {
    let (_home, store) = temp_store();
    let cwd = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let id = "public-harvested";
    let dir = store.directory(id).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("meta.json"),
        json!({
            "id": id,
            "cwd": cwd.path(),
            "status": "completed",
            "mode": "browser",
            "createdAt": "2024-01-01T00:00:00.000Z"
        })
        .to_string(),
    )
    .unwrap();
    let destination = out.path().join("answer.md");
    let root = parse(&[
        "session",
        id,
        "--harvest",
        "--write-output",
        destination.to_str().unwrap(),
    ]);
    let view = root.subcommand_matches("session").unwrap();
    print_inspected_answer(
        &store,
        id,
        &root,
        view,
        "PUBLIC harvested answer",
        &CancellationToken::new(),
    )
    .unwrap();
    let written = std::fs::read_to_string(&destination).unwrap();
    assert!(written.contains("PUBLIC harvested answer"), "{written:?}");
}

#[test]
fn route_without_models_plans_the_default_model() {
    let env = config::runtime_environment();
    for preflight in [false, true] {
        let implicit = route(&json!({}), &env, preflight).unwrap();
        let explicit = route(
            &json!({"models": [crate::policy::DEFAULT_API_MODEL]}),
            &env,
            preflight,
        )
        .unwrap();
        assert_eq!(implicit, explicit);
        assert!(matches!(implicit, 0 | 1));
    }
}

#[test]
fn doctor_refuses_azure_with_no_azure_and_no_azure_overrides_provider() {
    let (_home, store) = temp_store();
    let cwd = tempfile::tempdir().unwrap();
    let env = config::runtime_environment();
    let conflicting = parse(&["doctor", "--providers", "--provider", "azure", "--no-azure"]);
    let sub = conflicting.subcommand_matches("doctor").unwrap();
    assert_eq!(
        doctor(&store, &conflicting, sub, &env, cwd.path())
            .unwrap_err()
            .to_string(),
        "--provider azure cannot be combined with --no-azure."
    );
    let named = parse(&[
        "doctor",
        "--providers",
        "--provider",
        "openai",
        "--no-azure",
    ]);
    let named_code = doctor(
        &store,
        &named,
        named.subcommand_matches("doctor").unwrap(),
        &env,
        cwd.path(),
    )
    .unwrap();
    let bare = parse(&["doctor", "--providers", "--no-azure"]);
    let bare_code = doctor(
        &store,
        &bare,
        bare.subcommand_matches("doctor").unwrap(),
        &env,
        cwd.path(),
    )
    .unwrap();
    assert_eq!(named_code, bare_code);
    assert!(matches!(named_code, 0 | 1));
}

#[test]
fn copy_with_uses_the_first_successful_command() {
    copy_with(&[("cat", &[])], "PUBLIC clipboard").unwrap();
    // A command that exits without reading breaks the pipe; its success still counts.
    copy_with(&[("true", &[])], &"x".repeat(4 * 1024 * 1024)).unwrap();
    assert_eq!(
        copy_with(
            &[("public-missing-clipboard-command", &[]), ("false", &[])],
            "PUBLIC clipboard"
        )
        .unwrap_err()
        .to_string(),
        "no usable native clipboard command"
    );
}
