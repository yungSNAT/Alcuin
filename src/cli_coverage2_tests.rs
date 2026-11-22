//! Further coverage tests for CLI entry points, serve and project-sources validation, route
//! checks, browser duration overrides and tab-scoped session inspections.

use super::*;

fn parse(args: &[&str]) -> ArgMatches {
    let mut argv = vec!["alcuin"];
    argv.extend_from_slice(args);
    parser()
        .try_get_matches_from(argv)
        .unwrap_or_else(|error| panic!("{args:?}: {error}"))
}

fn temp_store() -> (tempfile::TempDir, SessionStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("oracle-home"));
    (dir, store)
}

#[cfg(unix)]
fn os_args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

#[cfg(unix)]
#[tokio::test]
async fn run_refuses_non_utf8_arguments_and_malformed_keeper_identity() {
    use std::os::unix::ffi::OsStringExt;

    // Non-UTF-8 arguments bypass trace normalisation and are refused by the parser.
    let mut args = os_args(&["alcuin", "--model"]);
    args.push(OsString::from_vec(b"PUBLIC-\xff".to_vec()));
    assert_eq!(run(args).await.unwrap(), 1);

    // The keeper identity is parsed as JSON before the keeper starts.
    let error = run(os_args(&[
        "alcuin",
        "--exec-window-keeper",
        "PUBLIC-not-json",
        "--keeper-ready",
        "PUBLIC-ready",
    ]))
    .await
    .unwrap_err();
    assert!(
        error.downcast_ref::<serde_json::Error>().is_some(),
        "{error:#}"
    );
}

#[tokio::test]
async fn docs_check_prints_a_text_report_for_explicit_paths() {
    let (_home, store) = temp_store();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(
        cwd.path().join("PUBLIC-guide.md"),
        "# PUBLIC guide\n\nPUBLIC text.\n",
    )
    .unwrap();
    let m = parse(&["docs", "check", "--docs-path", "PUBLIC-guide.md"]);
    let sub = m.subcommand_matches("docs").unwrap();
    let child = sub.subcommand_matches("check").unwrap();
    let paths = values(child, "docs-path");
    assert_eq!(paths, ["PUBLIC-guide.md"]);
    let cwd_path = cwd.path().to_path_buf();
    let expected = crate::docs_check::check(&cwd_path, &paths);
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
async fn serve_reads_the_profile_directory_then_refuses_both_token_sources() {
    let (_home, store) = temp_store();
    let cwd = tempfile::tempdir().unwrap();
    let profile = cwd.path().join("PUBLIC-profile");
    std::fs::create_dir_all(&profile).unwrap();
    let m = parse(&[
        "serve",
        "--port",
        "0",
        "--manual-login-profile-dir",
        profile.to_str().unwrap(),
        "--token",
        "PUBLIC-token",
        "--token-file",
        "PUBLIC-token-file",
    ]);
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
    assert_eq!(error.to_string(), "Use --token or --token-file, not both.");
}

#[tokio::test]
async fn project_sources_refuses_an_invalid_program_browser_port() {
    let (_home, store) = temp_store();
    let cwd = tempfile::tempdir().unwrap();
    let m = parse(&["--browser-port", "0", "project-sources", "list"]);
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
    assert_eq!(
        error.to_string(),
        "Browser port must be an integer between 1 and 65535."
    );
}

#[test]
fn route_check_deduplicates_requested_models_like_the_single_model_flag() {
    let (_home, store) = temp_store();
    let cwd = tempfile::tempdir().unwrap();
    let env = config::runtime_environment();
    let model = crate::policy::DEFAULT_API_MODEL;
    let m = parse(&["--engine", "api", "--model", model]);
    let (input, followup) = root_input(
        &store,
        &m,
        &env,
        cwd.path(),
        Some("PUBLIC prompt".into()),
        vec![],
        false,
    )
    .unwrap();
    assert!(followup.is_none());
    let loaded = load_config(&store, cwd.path(), &env, false);
    let request = execution::resolve(&input, &loaded.config, &env, cwd.path()).unwrap();

    let mut single = input.clone();
    if let Some(object) = single.as_object_mut() {
        object.remove("models");
    }
    let implicit = route_check(&single, &request, &loaded, &m, &env).unwrap();

    let mut listed = input.clone();
    listed["models"] = json!([model, model]);
    let explicit = route_check(&listed, &request, &loaded, &m, &env).unwrap();

    assert_eq!(implicit, explicit);
    assert!(matches!(explicit, 0 | 1), "{explicit}");
}

#[test]
fn browser_timeouts_beyond_the_integer_range_stay_floating_point() {
    let env = config::runtime_environment();
    let huge = parse(&["--browser-timeout", "100000000000000000000000h"]);
    let overrides = browser_runtime_overrides_at_scope(&huge, &[], &json!({}), &env).unwrap();
    let value = &overrides["timeoutMs"];
    assert!(value.as_u64().is_none(), "{value}");
    assert!(value.as_f64().unwrap() >= u64::MAX as f64, "{value}");

    let small = parse(&["--browser-timeout", "2s"]);
    let overrides = browser_runtime_overrides_at_scope(&small, &[], &json!({}), &env).unwrap();
    assert_eq!(overrides["timeoutMs"], 2000);
}

#[tokio::test]
async fn tab_scoped_inspection_of_a_missing_session_fails_without_a_saved_answer() {
    let (_home, store) = temp_store();
    let env = config::runtime_environment();
    let id = "public-missing-inspection";
    for mode in ["--live", "--harvest"] {
        let root = parse(&["--browser-tab", "PUBLIC-tab", "session", id, mode]);
        let view = root.subcommand_matches("session").unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            inspect_session(&store, id, &root, view, &env),
        )
        .await
        .expect("inspection finished");
        let error = result.unwrap_err();
        let message = format!("{error:#}");
        assert!(!message.is_empty(), "{mode}");
        assert!(
            !message.contains("Browser inspection cancelled"),
            "{mode}: {message}"
        );
    }
}
