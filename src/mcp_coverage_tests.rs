use super::*;
use std::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn isolated_server() -> (tempfile::TempDir, AlcuinMcp) {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().join("project");
    fs::create_dir(&cwd).unwrap();

    let mut env = Environment::default();
    let home_path = home.path().to_string_lossy().into_owned();
    env.insert("HOME".to_owned(), home_path.clone());
    env.insert("PATH".to_owned(), home_path);

    let server = AlcuinMcp {
        store: SessionStore::new(home.path()),
        cwd,
        env,
    };
    (home, server)
}

#[tokio::test]
async fn log_forwarding_does_not_retry_after_stopping_or_read_past_a_stale_offset() {
    let (_home, server) = isolated_server();
    let meta = server
        .store
        .create(json!({"prompt":"PUBLIC-forwarding"}), &server.cwd, None)
        .unwrap();
    let id = meta["id"].as_str().unwrap();
    server
        .store
        .append_log(id, None, "PUBLIC-retained log line\n")
        .unwrap();
    let saved = server.store.log(id).unwrap();
    assert!(saved.contains("PUBLIC-retained log line"));

    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    LOG.scope(sender, async {
        let mut stopped = LogForward {
            bytes: MAX_FORWARDED_BYTES,
            ..LogForward::default()
        };
        stopped.step(&server.store, id, false);
        assert!(stopped.stopped);
        assert_eq!(
            receiver.try_recv().unwrap(),
            "Log forwarding stopped at its limit; the session keeps the whole log."
        );

        stopped.step(&server.store, id, true);
        assert_eq!(stopped.bytes, MAX_FORWARDED_BYTES);
        assert_eq!(stopped.offset, 0);
        assert_eq!(
            receiver.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        );

        let offset = saved.len() + 1;
        let mut stale = LogForward {
            offset,
            ..LogForward::default()
        };
        stale.step(&server.store, id, true);
        assert_eq!(stale.offset, offset);
        assert_eq!(stale.bytes, 0);
        assert!(!stale.stopped);
        assert_eq!(
            receiver.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        );
    })
    .await;
    assert_eq!(server.store.log(id).unwrap(), saved);
}

#[tokio::test]
async fn browser_consult_reports_unavailable_chrome_before_creating_a_session() {
    let (_home, server) = isolated_server();
    if crate::browser_detect::chrome(&server.env).is_some() {
        // A live browser consult is not safe on hosts with a discoverable Chrome.
        return;
    }

    let error = server
        .consult(
            json!({
                "engine":"browser",
                "model":"gpt-6-pro",
                "prompt":"PUBLIC-no-chrome"
            }),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Browser engine unavailable: no Chrome installation found and CHROME_PATH is unset."
    );
    assert!(
        server
            .store
            .list_created_since(true, None)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn project_sources_rejects_an_invalid_project_url_even_in_dry_run() {
    let (_home, server) = isolated_server();
    let url = "https://example.invalid/PUBLIC-not-a-project";
    let expected = project_sources::normalise_url(url).unwrap_err();
    let error = server
        .invoke(
            "project_sources",
            json!({"operation":"list","chatgptUrl":url,"dryRun":true}),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), expected.to_string());
}

#[test]
fn resource_uris_decode_components_and_serve_the_stored_request_without_a_request_file() {
    let (_home, server) = isolated_server();
    let meta = server
        .store
        .create(json!({"prompt":"PUBLIC-resource"}), &server.cwd, None)
        .unwrap();
    let id = meta["id"].as_str().unwrap();

    let uri = format!("oracle-session://{id}/%6Detadata");
    let resource = server.resource(&uri).unwrap();
    let entry = &resource["contents"][0];
    assert_eq!(entry["uri"], uri);
    let returned_meta: Value = serde_json::from_str(entry["text"].as_str().unwrap()).unwrap();
    assert_eq!(returned_meta["id"], id);

    let request_path = server.store.directory(id).unwrap().join("request.json");
    if request_path.exists() {
        fs::remove_file(request_path).unwrap();
    }
    let uri = format!("oracle-session://{id}/r%65quest");
    let resource = server.resource(&uri).unwrap();
    let entry = &resource["contents"][0];
    assert_eq!(entry["uri"], uri);
    let request: Value = serde_json::from_str(entry["text"].as_str().unwrap()).unwrap();
    assert_eq!(request["prompt"], "PUBLIC-resource");

    let unknown = server
        .resource(&format!("oracle-session://{id}/PUBLIC-%E2%98%83"))
        .unwrap_err();
    assert_eq!(unknown.to_string(), "Unsupported resource kind: PUBLIC-☃");
    assert_eq!(
        decode_component("PUBLIC-%2").unwrap_err().to_string(),
        "Invalid URI escape"
    );
    let non_hex = decode_component("PUBLIC-%GG").unwrap_err();
    assert_eq!(
        non_hex
            .downcast_ref::<std::num::ParseIntError>()
            .unwrap()
            .kind(),
        &std::num::IntErrorKind::InvalidDigit
    );
}

#[test]
fn summaries_report_pending_cleanup_only_for_completed_sessions() {
    for (status, warning, recovery, pending, expected) in [
        ("completed", None, true, true, true),
        ("completed", None, true, false, false),
        ("completed", None, false, true, false),
        ("completed", Some("PUBLIC-cleanup"), false, false, true),
        ("running", Some("PUBLIC-cleanup"), true, true, false),
    ] {
        let mut summary = json!({});
        add_summaries(
            &mut summary,
            &json!({
                "status":status,
                "cleanupWarning":warning,
                "recoveryRequired":recovery,
                "browser":{"runtime":{"targetDispositionPending":pending}}
            }),
        );
        if expected {
            assert_eq!(
                summary,
                json!({"browserCleanup":{"state":"pending","recoveryRequired":true}})
            );
        } else {
            assert_eq!(summary, json!({}));
        }
    }
}

#[test]
fn summaries_filter_model_errors_and_include_image_specific_fields_only_for_images() {
    let mut summary = json!({});
    add_summaries(
        &mut summary,
        &json!({
            "models":[{
                "model":"PUBLIC-model",
                "status":"error",
                "error":{
                    "category":"PUBLIC-category",
                    "message":"PUBLIC-message",
                    "debug":"PUBLIC-model-debug"
                }
            }],
            "artifacts":[
                {
                    "kind":"image",
                    "path":"PUBLIC-image.png",
                    "label":"PUBLIC-image",
                    "mimeType":"image/png",
                    "sizeBytes":128,
                    "alt":"PUBLIC-description",
                    "width":16,
                    "height":8,
                    "fileId":"PUBLIC-file-id",
                    "debug":"PUBLIC-image-debug"
                },
                {
                    "kind":"file",
                    "path":"PUBLIC-report.txt",
                    "alt":"PUBLIC-not-an-image",
                    "fileId":"PUBLIC-report-id"
                }
            ]
        }),
    );
    assert_eq!(
        summary,
        json!({
            "models":[{
                "model":"PUBLIC-model",
                "status":"error",
                "error":{"category":"PUBLIC-category","message":"PUBLIC-message"}
            }],
            "artifacts":[
                {
                    "kind":"image","path":"PUBLIC-image.png","label":"PUBLIC-image",
                    "mimeType":"image/png","sizeBytes":128
                },
                {"kind":"file","path":"PUBLIC-report.txt"}
            ],
            "images":[{
                "kind":"image","path":"PUBLIC-image.png","label":"PUBLIC-image",
                "mimeType":"image/png","sizeBytes":128,"alt":"PUBLIC-description",
                "width":16,"height":8,"fileId":"PUBLIC-file-id"
            }]
        })
    );
}

#[test]
fn input_validation_honors_constraints_without_a_type_and_counts_utf16_units() {
    let untyped = json!({"enum":["PUBLIC-allowed",false]});
    for value in [json!("PUBLIC-allowed"), json!(false)] {
        assert!(validate_input(&value, &untyped, "input.choice").is_ok());
    }
    assert_eq!(
        validate_input(&json!("PUBLIC-other"), &untyped, "input.choice")
            .unwrap_err()
            .to_string(),
        "input.choice: invalid enum value"
    );

    let schema = json!({
        "type":"object",
        "properties":{"labels":{"type":"array","items":{"type":"string","minLength":9}}}
    });
    assert!(validate_input(&json!({"labels":["PUBLIC-😀"]}), &schema, "input").is_ok());
    assert_eq!(
        validate_input(
            &json!({"labels":["PUBLIC-😀","PUBLIC-x"]}),
            &schema,
            "input"
        )
        .unwrap_err()
        .to_string(),
        "input.labels[1]: must contain at least 9 character(s)"
    );
}

async fn send_rpc(writer: &mut (impl tokio::io::AsyncWrite + Unpin), message: Value) {
    let mut bytes = serde_json::to_vec(&message).unwrap();
    bytes.push(b'\n');
    writer.write_all(&bytes).await.unwrap();
    writer.flush().await.unwrap();
}

async fn rpc_request(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    send_rpc(
        writer,
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
    .await;
    let mut line = String::new();
    assert_ne!(
        reader.read_line(&mut line).await.unwrap(),
        0,
        "{method}: EOF"
    );
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["id"], json!(id), "{method}: {response}");
    response
}

struct RestoreLogLevel(u8);

impl Drop for RestoreLogLevel {
    fn drop(&mut self) {
        LOG_LEVEL.store(self.0, Ordering::Relaxed);
    }
}

#[tokio::test]
async fn mcp_dispatch_lists_resources_rejects_unknown_tools_and_handles_level_and_task_updates() {
    let (_home, server) = isolated_server();

    tokio::time::timeout(Duration::from_secs(3), async {
        let (client_pipe, server_pipe) = tokio::io::duplex(16 * 1024);
        let (server_reader, server_writer) = tokio::io::split(server_pipe);
        let start =
            tokio::spawn(
                async move { server.serve((server_reader, server_writer)).await.unwrap() },
            );
        let (client_reader, mut writer) = tokio::io::split(client_pipe);
        let mut reader = BufReader::new(client_reader);

        let initialized = rpc_request(
            &mut reader,
            &mut writer,
            1,
            "initialize",
            json!({
                "protocolVersion":"2025-11-25",
                "capabilities":{"extensions":{"io.modelcontextprotocol/tasks":{}}},
                "clientInfo":{"name":"PUBLIC-coverage-client","version":"1.0.0"}
            }),
        )
        .await;
        assert_eq!(
            initialized["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        send_rpc(
            &mut writer,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .await;
        let running = start.await.unwrap();

        let resources = rpc_request(&mut reader, &mut writer, 2, "resources/list", json!({})).await;
        assert_eq!(resources["result"]["resources"], json!([]));

        let unknown = rpc_request(
            &mut reader,
            &mut writer,
            3,
            "tools/call",
            json!({"name":"PUBLIC-unknown-tool","arguments":{}}),
        )
        .await;
        assert_eq!(unknown["error"]["code"], json!(-32602));
        assert_eq!(unknown["error"]["message"], "Unknown MCP tool");
        assert!(unknown.get("result").is_none());

        let _restore = RestoreLogLevel(LOG_LEVEL.load(Ordering::Relaxed));
        // Both levels leave info notifications enabled for parallel forwarding tests.
        #[allow(deprecated)]
        let levels = [
            (4, "debug", LoggingLevel::Debug),
            (5, "info", LoggingLevel::Info),
        ];
        for (id, name, level) in levels {
            let response = rpc_request(
                &mut reader,
                &mut writer,
                id,
                "logging/setLevel",
                json!({"level":name}),
            )
            .await;
            assert!(response.get("result").is_some(), "{response}");
            assert_eq!(LOG_LEVEL.load(Ordering::Relaxed), level as u8 + 1);
        }

        let task_id = "PUBLIC-nonexistent-task";
        let update = rpc_request(
            &mut reader,
            &mut writer,
            6,
            "tasks/update",
            json!({"taskId":task_id,"inputResponses":{}}),
        )
        .await;
        assert_eq!(update["error"]["code"], json!(-32602));
        assert_eq!(
            update["error"]["message"],
            format!("unknown task: {task_id}")
        );
        assert!(update.get("result").is_none());

        running.cancellation_token().cancel();
        running.waiting().await.unwrap();
    })
    .await
    .expect("in-memory MCP exchange timed out");
}
