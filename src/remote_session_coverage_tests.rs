use super::*;
use crate::browser_prompt::{BundleFormat, BundleScope, PendingBundle};
use http_body_util::Full;
use hyper::{Response, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use std::{convert::Infallible, fs, future::ready, path::PathBuf, sync::OnceLock, time::Duration};
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};

const RUN_ID: &str = "12345678-1234-1234-1234-123456789abc";
const TOKEN: &str = "PUBLIC-loopback-token";
const DEADLINE: Duration = Duration::from_secs(4);

fn offline_connection() -> Connection {
    Connection {
        host: "127.0.0.1:1".into(),
        token: None,
    }
}

fn browser_request(cwd: &Path) -> ResolvedRequest {
    let mut request = crate::execution::resolve(
        &json!({"prompt":"PUBLIC-receiver-prompt","engine":"browser","model":"gpt-6-pro"}),
        &json!({}),
        &Environment::new(),
        cwd,
    )
    .unwrap();
    request.options["browserConfig"] = request.browser.clone();
    request
}

fn start_session(
    store: &SessionStore,
    cwd: &Path,
    request: &ResolvedRequest,
    connection: &Connection,
) -> (String, ProcessIdentity) {
    let metadata = prepare(store, request, cwd, connection).unwrap();
    let id = metadata["id"].as_str().unwrap().to_owned();
    let identity = ProcessIdentity::current().unwrap();
    crate::execution::start_owner(store, &id, &identity, false).unwrap();
    (id, identity)
}

#[test]
fn worker_environment_replaces_the_host_and_does_not_inherit_a_stale_token() {
    let original = Environment::from_iter([
        ("ORACLE_REMOTE_HOST".into(), "127.0.0.1:1234".into()),
        ("ORACLE_REMOTE_TOKEN".into(), "PUBLIC-stale-token".into()),
        ("ALCUIN_FIXTURE".into(), "PUBLIC-preserved-value".into()),
    ]);

    for token in [Some(TOKEN), None] {
        let connection = Connection {
            host: "127.0.0.1:4321".into(),
            token: token.map(str::to_owned),
        };
        let worker = connection.worker_environment(&original);
        assert_eq!(
            worker.get("ORACLE_REMOTE_HOST").map(String::as_str),
            Some(connection.host.as_str())
        );
        assert_eq!(worker.get("ORACLE_REMOTE_TOKEN").map(String::as_str), token);
        assert_eq!(
            worker.get("ALCUIN_FIXTURE").map(String::as_str),
            Some("PUBLIC-preserved-value")
        );
    }

    assert_eq!(
        original.get("ORACLE_REMOTE_TOKEN").map(String::as_str),
        Some("PUBLIC-stale-token")
    );
    assert_eq!(
        original.get("ORACLE_REMOTE_HOST").map(String::as_str),
        Some("127.0.0.1:1234")
    );
}

#[test]
fn restart_commits_the_admitted_host_without_persisting_ephemeral_credentials() {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path();
    let store = SessionStore::new(cwd.join("receiver"));
    let mut request = browser_request(cwd);
    let (source, identity) = start_session(&store, cwd, &request, &offline_connection());
    Receiver::new(&store, &source, &identity, false, &CancellationToken::new())
        .unwrap()
        .captured(json!({"answerText":"PUBLIC-source-answer"}))
        .unwrap();

    let ephemeral = [
        ("remoteToken", "PUBLIC-request-token"),
        ("inlineCookies", "PUBLIC-request-cookies"),
        ("inlineCookiesSource", "PUBLIC-request-cookie-source"),
    ];
    for (key, value) in ephemeral {
        request.options[key] = json!(value);
        request.browser[key] = json!(value);
    }
    request.browser["remoteHost"] = json!("127.0.0.1:1234");
    request.options["browserConfig"] = request.browser.clone();
    let connection = Connection {
        host: "127.0.0.1:4321".into(),
        token: Some(TOKEN.into()),
    };

    let restarted = prepare_restart(&store, &request, cwd, &connection, &source).unwrap();
    let id = restarted["id"].as_str().unwrap();
    assert_ne!(id, source);
    let saved = store.require(id).unwrap();
    assert_eq!(saved["status"], "pending");
    assert_eq!(
        saved["options"]["browserConfig"]["remoteHost"],
        connection.host
    );
    for name in ["meta.json", "browser-request.json"] {
        let durable = fs::read_to_string(store.directory(id).unwrap().join(name)).unwrap();
        for (_, value) in ephemeral {
            assert!(!durable.contains(value), "{name} retained {value}");
        }
        assert!(
            !durable.contains(TOKEN),
            "{name} retained the connection token"
        );
    }
    let outgoing = payload(&store, id).unwrap();
    assert!(
        outgoing["prompt"]
            .as_str()
            .unwrap()
            .contains("PUBLIC-receiver-prompt")
    );
    assert!(outgoing["browserConfig"]["remoteHost"].is_null());
    assert_eq!(store.require(&source).unwrap()["status"], "completed");
}

fn install_fallback(
    store: &SessionStore,
    id: &str,
    format: BundleFormat,
    scope: BundleScope,
    uploads: &[Value],
) -> StagedUploads {
    let staged = StagedUploads::capture_remote(store, id, uploads).unwrap();
    let mut committed = browser_runner::request(store, id).unwrap();
    committed.fallback = Some(browser_runner::BrowserFallback {
        composer_text: "PUBLIC-fallback-prompt".into(),
        pending_bundle: Some(PendingBundle { format, scope }),
        text_sources: vec![],
        raw_source: None,
        remote_source: Some(staged.clone()),
    });
    crate::storage::atomic_json(
        &store.directory(id).unwrap().join("browser-request.json"),
        &committed,
    )
    .unwrap();
    staged
}

fn wire_file(name: &str, bytes: &[u8]) -> Value {
    json!({
        "fileName": name,
        "displayPath": format!("PUBLIC-inputs/{name}"),
        "contentBase64": STANDARD.encode(bytes),
    })
}

#[test]
fn receiver_rejects_an_invalid_run_hint_and_retains_answer_output_with_no_downloads() {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path();
    let store = SessionStore::new(cwd.join("receiver"));
    let output_dir = cwd.join("PUBLIC-output");
    fs::create_dir(&output_dir).unwrap();
    let mut request = browser_request(cwd);
    request.options["writeOutputPath"] = json!("PUBLIC-output/PUBLIC-answer.md");
    request.options["writeArtifacts"] = json!(true);
    let connection = offline_connection();
    let (id, identity) = start_session(&store, cwd, &request, &connection);
    let receiver = Receiver::new(&store, &id, &identity, false, &CancellationToken::new()).unwrap();

    let invalid = "../PUBLIC-invalid-run";
    let error = receiver
        .hint(json!({"remoteHost": connection.host, "remoteRunId": invalid}))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        crate::remote_result::session_id(invalid)
            .unwrap_err()
            .to_string()
    );
    assert!(store.require(&id).unwrap()["browser"]["runtime"]["remoteRunId"].is_null());
    receiver
        .hint(json!({"remoteHost": connection.host, "remoteRunId": RUN_ID}))
        .unwrap();
    assert_eq!(
        store.require(&id).unwrap()["browser"]["runtime"]["remoteRunId"],
        RUN_ID
    );

    let markdown = "PUBLIC-**formatted answer**";
    receiver
        .captured(json!({
            "answerText": "PUBLIC-plain-answer",
            "answerMarkdown": markdown,
            "savedFiles": [],
        }))
        .unwrap();

    let saved = store.require(&id).unwrap();
    assert_eq!(saved["status"], "completed");
    let capture = serde_json::from_value(saved["capturedAnswer"].clone()).unwrap();
    assert_eq!(
        artifacts::verify_capture(&store, &capture).unwrap(),
        markdown.as_bytes()
    );

    let answer = output_dir.join("PUBLIC-answer.md");
    let receipt = &saved["browser"]["requiredTextOutput"];
    assert_eq!(receipt["file"], json!(answer));
    assert!(fs::read_to_string(answer).unwrap().contains(markdown));
    assert!(
        saved["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|artifact| {
                artifact["kind"] == "output"
                    && artifact["path"] == receipt["file"]
                    && artifact["bytes"] == receipt["bytes"]
                    && artifact["sha256"] == receipt["sha256"]
            }),
        "missing output receipt: {}",
        saved["artifacts"]
    );
}

async fn serve_one_response(
    status: u16,
    body: Value,
    before_response: impl Fn() + Send + Sync + 'static,
) -> (Connection, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let connection = Connection {
        host: listener.local_addr().unwrap().to_string(),
        token: Some(TOKEN.into()),
    };
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        drop(listener);
        let body = bytes::Bytes::from(body.to_string());
        let service = service_fn(move |_| {
            before_response();
            ready(Ok::<_, Infallible>(
                Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .header("connection", "close")
                    .body(Full::new(body.clone()))
                    .unwrap(),
            ))
        });
        http1::Builder::new()
            .serve_connection(TokioIo::new(stream), service)
            .await
            .unwrap();
    });
    (connection, task)
}

#[tokio::test]
async fn legacy_health_reply_requires_local_fallback_and_retains_both_dispatch_errors() {
    timeout(DEADLINE, async {
        let home = tempfile::tempdir().unwrap();
        let cwd = home.path();
        let store = SessionStore::new(cwd.join("receiver"));
        let mutation = Arc::new(OnceLock::<(String, PathBuf)>::new());
        let server_mutation = mutation.clone();
        let server_store = store.clone();
        let (connection, server) = serve_one_response(
            200,
            json!({"ok":true,"capabilities":{"deferredFallbackBundling":false}}),
            move || {
                let (id, path) = server_mutation.get().unwrap();
                fs::write(path, b"PUBLIC-replaced-fallback-source").unwrap();
                server_store
                    .update(id, |meta| {
                        let runtime = merge_object(
                            &meta["browser"]["runtime"],
                            &json!({"remoteRunId": RUN_ID}),
                        );
                        let browser = merge_object(&meta["browser"], &json!({"runtime": runtime}));
                        Ok(Some(json!({"browser": browser})))
                    })
                    .unwrap();
            },
        )
        .await;
        let (id, identity) = start_session(&store, cwd, &browser_request(cwd), &connection);
        let source = install_fallback(
            &store,
            &id,
            BundleFormat::Zip,
            BundleScope::All,
            &[wire_file("PUBLIC-source.txt", b"PUBLIC-original-source")],
        );
        mutation
            .set((id.clone(), source.paths().unwrap().remove(0)))
            .unwrap();

        let error = execute(
            &store,
            &id,
            &connection,
            &identity,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        server.await.unwrap();
        assert!(
            error
                .to_string()
                .contains("Receiver finalisation also failed")
        );
        assert!(
            error
                .to_string()
                .contains("options or run identity changed")
        );
        let source_error = source.verify(&store, &id).unwrap_err();
        let details = format!("{error:#}");
        assert!(details.contains(&format!("{source_error:#}")), "{details}");
        let saved = store.require(&id).unwrap();
        assert_eq!(saved["status"], "running");
        assert_eq!(saved["browser"]["runtime"]["remoteRunId"], RUN_ID);
        assert!(saved["browser"]["remoteDispatch"]["attemptId"].is_string());
        assert!(saved["browser"]["remoteResult"].is_null());
    })
    .await
    .expect("legacy health and receiver finalisation timed out");
}

#[tokio::test]
async fn precancelled_dispatch_persists_cancelled_status_without_capturing_a_result() {
    timeout(DEADLINE, async {
        let home = tempfile::tempdir().unwrap();
        let cwd = home.path();
        let driver: crate::remote_host::RunDriver = Arc::new(|_| {
            Box::pin(async { anyhow::bail!("PUBLIC-producer-fixture-does-not-run-a-browser") })
        });
        let host = crate::remote_host::RemoteHost::start(
            SessionStore::new(cwd.join("producer")),
            crate::remote_host::HostOptions {
                address: "127.0.0.1:0".parse().unwrap(),
                token: TOKEN.into(),
                max_concurrent_runs: Some(1),
                max_queued_runs: Some(0),
                max_browser_tabs: 1,
            },
            driver,
        )
        .await
        .unwrap();
        let connection = Connection {
            host: host.address.to_string(),
            token: Some(TOKEN.into()),
        };
        let store = SessionStore::new(cwd.join("receiver"));
        let (id, identity) = start_session(&store, cwd, &browser_request(cwd), &connection);

        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = execute(&store, &id, &connection, &identity, &cancel)
            .await
            .unwrap_err();
        let saved = store.require(&id).unwrap();
        assert_eq!(saved["status"], "cancelled");
        assert_eq!(saved["recoveryRequired"], true);
        assert_eq!(saved["errorMessage"], format!("{error:#}"));
        assert!(saved["browser"]["remoteResult"].is_null());
        assert_eq!(store.model_runs(&id).unwrap()[0]["status"], "cancelled");
        host.shutdown().await.unwrap();
    })
    .await
    .expect("cancelled loopback dispatch timed out");
}

#[tokio::test]
async fn recovery_retains_a_completed_capture_or_reports_refused_failure_persistence() {
    timeout(DEADLINE, async {
        for capture_before_failure in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let store = SessionStore::new(home.path().join("receiver"));
            let server_store = store.clone();
            let (connection, server) = serve_one_response(
                500,
                json!({"error":"PUBLIC-recovery-transport-error"}),
                move || {
                    let sessions = server_store.list(true).unwrap();
                    assert_eq!(sessions.len(), 1);
                    let id = sessions[0]["id"].as_str().unwrap();
                    if capture_before_failure {
                        Receiver::new(
                            &server_store,
                            id,
                            &ProcessIdentity::current().unwrap(),
                            true,
                            &CancellationToken::new(),
                        )
                        .unwrap()
                        .captured(json!({"answerText": "PUBLIC-recovered-answer"}))
                        .unwrap();
                    } else {
                        server_store
                            .update(id, |meta| {
                                let options =
                                    merge_object(&meta["options"], &json!({"verbose":true}));
                                Ok(Some(json!({"options": options})))
                            })
                            .unwrap();
                    }
                },
            )
            .await;

            let error = recover(
                &store,
                home.path(),
                &connection,
                RUN_ID,
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
            server.await.unwrap();
            let sessions = store.list(true).unwrap();
            assert_eq!(sessions.len(), 1);
            let id = sessions[0]["id"].as_str().unwrap();
            let retained = format!("Remote recovery receiver session {id} is retained");
            assert_eq!(error.to_string(), retained);
            let details = format!("{error:#}");
            let saved = store.require(id).unwrap();
            assert_eq!(saved["browser"]["runtime"]["remoteRunId"], RUN_ID);
            assert!(saved["errorMessage"].is_null());

            if capture_before_failure {
                assert_eq!(saved["status"], "completed");
                assert_eq!(store.model_runs(id).unwrap()[0]["status"], "completed");
                assert_eq!(saved["recoveryRequired"], true);
                let handoff_error = saved["browser"]["remoteHandoffError"].as_str().unwrap();
                assert!(!handoff_error.is_empty());
                assert_eq!(details, format!("{retained}: {handoff_error}"));
                let capture = serde_json::from_value(saved["capturedAnswer"].clone()).unwrap();
                assert_eq!(
                    artifacts::verify_capture(&store, &capture).unwrap(),
                    b"PUBLIC-recovered-answer"
                );
            } else {
                assert!(
                    details.contains("Recovery failure persistence also failed"),
                    "{details}"
                );
                assert!(
                    details.contains("options or run identity changed"),
                    "{details}"
                );
                assert_eq!(saved["status"], "running");
                assert_eq!(saved["options"]["verbose"], true);
                assert!(saved["browser"]["remoteResult"].is_null());
            }
        }
    })
    .await
    .expect("loopback recovery timed out");
}
