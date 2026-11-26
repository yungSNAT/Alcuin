use super::*;
use std::{fs, time::Duration};

fn isolated_environment(home: &Path) -> Environment {
    Environment::from([("HOME".into(), home.to_string_lossy().into_owned())])
}

fn browser_metadata() -> Value {
    json!({
        "id": "PUBLIC-browser-source",
        "mode": "browser",
        "options": {
            "prompt": "PUBLIC saved browser prompt",
            "model": "gpt-6-pro",
            "file": []
        },
        "browser": {
            "config": {
                "desiredModel": "Latest",
                "thinkingTime": "pro",
                "modelStrategy": "select",
                "chatgptUrl": "https://chatgpt.com/",
                "cookieSync": false,
                "keepBrowser": true
            }
        }
    })
}

#[test]
fn browser_restart_discards_only_the_search_opt_out() {
    let home = tempfile::tempdir().unwrap();
    let environment = isolated_environment(home.path());

    for (saved_search, expected_search) in [(false, None), (true, Some(json!(true)))] {
        let mut metadata = browser_metadata();
        metadata["options"]["search"] = json!(saved_search);

        let (request, cwd) = saved_request(&metadata, home.path(), &environment).unwrap();

        assert_eq!(cwd, home.path());
        assert_eq!(request.engine, "browser");
        assert_eq!(request.browser["desiredModel"], "Latest");
        assert_eq!(
            request.options.get("search"),
            expected_search.as_ref(),
            "saved search={saved_search}"
        );
    }
}

#[tokio::test]
async fn api_restart_defaults_the_saved_model_and_reports_a_missing_key_after_config_warning() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let environment = isolated_environment(home.path());
    let store = SessionStore::new(home.path());
    let metadata = json!({
        "id": "PUBLIC-api-source",
        "mode": "api",
        "options": {
            "prompt": "PUBLIC saved API prompt",
            "reasoningMode": "pro",
            "file": []
        }
    });

    let (request, saved_cwd) = saved_request(&metadata, cwd.path(), &environment).unwrap();
    assert_eq!(request.options["model"], policy::DEFAULT_API_MODEL);
    assert_eq!(request.options["reasoningMode"], "pro");

    let (route, key) =
        providers::resolve_route(policy::DEFAULT_API_MODEL, &request.options, &environment)
            .unwrap();
    assert!(
        key.is_none(),
        "the isolated environment must not supply a key"
    );
    let expected_error = providers::missing_key_message(&route);
    let source =
        execution::prepare_restart(&store, &request, &saved_cwd, route, "PUBLIC-api-source")
            .unwrap();
    let source_id = source["id"].as_str().unwrap();

    fs::write(home.path().join("config.json"), "{ publicFixture:").unwrap();
    let loaded = config::load(home.path(), cwd.path(), Some(home.path()), true);
    assert!(!loaded.warnings.is_empty(), "malformed config should warn");

    let error = tokio::time::timeout(
        Duration::from_secs(3),
        run(
            &store,
            Options {
                source: source_id,
                wait: Some(true),
                remote_host: None,
                remote_token: None,
            },
            &environment,
            cwd.path(),
            &CancellationToken::new(),
        ),
    )
    .await
    .expect("restart should reject missing credentials before dispatch")
    .unwrap_err();

    assert_eq!(error.to_string(), expected_error);
}

#[tokio::test]
async fn remote_restart_overrides_configured_host_and_persists_forced_wait() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let environment = isolated_environment(home.path());
    let store = SessionStore::new(home.path());

    let (request, saved_cwd) =
        saved_request(&browser_metadata(), cwd.path(), &environment).unwrap();
    let source = crate::browser_runner::prepare_restart(
        &store,
        &request,
        &saved_cwd,
        "PUBLIC-browser-source",
    )
    .unwrap();
    let source_id = source["id"].as_str().unwrap();

    // If the host override is ignored, the fallback still points at a local listener.
    let fallback = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let fallback_host = fallback.local_addr().unwrap().to_string();
    fs::write(
        home.path().join("config.json"),
        json!({
            "browser": {
                "remoteHost": fallback_host,
                "remoteToken": "PUBLIC-fallback-token"
            }
        })
        .to_string(),
    )
    .unwrap();
    let loaded = config::load(home.path(), cwd.path(), Some(home.path()), true);
    assert_eq!(loaded.config["browser"]["remoteHost"], fallback_host);

    let server = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let host = format!("  {}  ", server.local_addr().unwrap());
    let serve_rejection = async move {
        let (socket, _) = server.accept().await.unwrap();
        let service =
            hyper::service::service_fn(|_: hyper::Request<hyper::body::Incoming>| async {
                let response = hyper::Response::builder()
                    .status(hyper::StatusCode::UNAUTHORIZED)
                    .body(http_body_util::Full::new(bytes::Bytes::from_static(
                        b"PUBLIC remote fixture rejected the request",
                    )))
                    .unwrap();
                Ok::<_, std::convert::Infallible>(response)
            });
        hyper::server::conn::http1::Builder::new()
            .keep_alive(false)
            .serve_connection(hyper_util::rt::TokioIo::new(socket), service)
            .await
            .unwrap();
    };

    let cancel = CancellationToken::new();
    let restart = run(
        &store,
        Options {
            source: source_id,
            wait: Some(false),
            remote_host: Some(&host),
            remote_token: Some("  PUBLIC-restart-token  "),
        },
        &environment,
        cwd.path(),
        &cancel,
    );

    let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(restart, serve_rejection)
    })
    .await
    .expect("restart should contact the override listener and handle its rejection");
    assert!(!matches!(result, Ok(0)), "{result:?}");

    let restarted: Vec<_> = fs::read_dir(home.path().join("sessions"))
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let id = name.to_str().unwrap();
            if id == source_id || !entry.file_type().unwrap().is_dir() {
                return None;
            }
            store.read(id).unwrap()
        })
        .collect();

    assert_eq!(
        restarted.len(),
        1,
        "restart should create one separate session"
    );
    assert_eq!(restarted[0]["mode"], "browser");
    assert_eq!(
        restarted[0]["options"]["prompt"],
        source["options"]["prompt"]
    );
    assert_eq!(
        restarted[0]["options"]["waitPreference"].as_bool(),
        Some(true),
        "remote restarts must wait even when the caller asks not to"
    );
}
