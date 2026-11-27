use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Serve `/json/version`-style answers with a fixed JSON body on a loopback port.
async fn serve_json(body: Value) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = vec![0; 4096];
            let _ = stream.read(&mut request).await;
            let body = body.to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes()).await;
        }
    });
    (port, server)
}

/// A loopback port with nothing listening on it.
async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

#[tokio::test]
async fn session_endpoint_without_a_usable_port_is_absent_without_io() {
    let cancel = CancellationToken::new();
    let cases = [
        json!({"browser":{"runtime":{"chromeHost":"127.0.0.1","chromePort":0}}}),
        json!({"browser":{"runtime":{"chromeHost":"127.0.0.1","chromePort":70000}}}),
        json!({"browser":{"runtime":{"chromeHost":"127.0.0.1","chromePort":-1}}}),
        json!({"browser":{"runtime":{"chromeHost":"127.0.0.1","chromePort":"9222"}}}),
        json!({"browser":{"runtime":{"chromeHost":"127.0.0.1"}}}),
        json!({"browser":{"runtime":{"chromeHost":"127.0.0.1","chromePort":null},"config":{"remoteChrome":{"port":0}}}}),
        json!({"browser":{"config":{"remoteChrome":{"host":"127.0.0.1","port":"PUBLIC-port"}}}}),
    ];
    for meta in cases {
        let found = session_endpoint(&meta, Duration::from_millis(200), &cancel)
            .await
            .unwrap();
        assert!(found.is_none(), "{meta}");
    }
}

#[tokio::test]
async fn session_endpoint_keeps_saved_socket_when_live_inspection_fails() {
    let port = closed_port().await;
    let saved = format!("ws://127.0.0.1:{port}/devtools/browser/PUBLIC-saved");
    let meta = json!({"browser":{"runtime":{
        "chromeHost":"127.0.0.1",
        "chromePort":port,
        "chromeBrowserWSEndpoint":saved.as_str()
    }}});
    let endpoint = session_endpoint(&meta, Duration::from_millis(200), &CancellationToken::new())
        .await
        .unwrap()
        .expect("saved endpoint");
    assert_eq!(endpoint.host, "127.0.0.1");
    assert_eq!(endpoint.port, port);
    assert_eq!(endpoint.browser_ws_endpoint, saved);
}

#[tokio::test]
async fn session_endpoint_refreshes_port_and_path_from_the_profile_active_port_file() {
    for key in ["chromeProfileRoot", "userDataDir"] {
        let profile = tempfile::tempdir().unwrap();
        std::fs::write(
            profile.path().join("DevToolsActivePort"),
            "9333\n/devtools/browser/PUBLIC-active\n",
        )
        .unwrap();
        let active = read_devtools_endpoint(profile.path())
            .unwrap()
            .expect("DevToolsActivePort should be readable");
        let host = active.host.clone();
        let saved = format!("ws://{host}:9444/devtools/browser/PUBLIC-saved");
        let mut meta = json!({"browser":{"runtime":{
            "chromeHost":host.as_str(),
            "chromePort":9444,
            "chromeBrowserWSEndpoint":saved.as_str()
        }}});
        meta["browser"]["runtime"][key] = json!(profile.path().to_str().unwrap());
        let endpoint =
            session_endpoint(&meta, Duration::from_millis(200), &CancellationToken::new())
                .await
                .unwrap()
                .expect("refreshed endpoint");
        let active_path = reqwest::Url::parse(&active.browser_ws_endpoint)
            .unwrap()
            .path()
            .to_owned();
        assert_eq!(endpoint.port, active.port, "{key}");
        assert_eq!(endpoint.host, host, "{key}");
        assert_eq!(
            endpoint.browser_ws_endpoint,
            format!("ws://{host}:{}{active_path}", active.port),
            "{key}"
        );
    }
}

#[tokio::test]
async fn session_endpoint_inspection_validates_the_advertised_socket() {
    let cancel = CancellationToken::new();
    let timeout = Duration::from_millis(1000);

    let (port, invalid) = serve_json(json!({"webSocketDebuggerUrl":"PUBLIC not a url"})).await;
    let meta = json!({"browser":{"runtime":{"chromeHost":"127.0.0.1","chromePort":port}}});
    let err = session_endpoint(&meta, timeout, &cancel).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("relative URL without a base"),
        "{err:#}"
    );
    invalid.abort();

    let (port, missing) = serve_json(json!({"Browser":"PUBLIC"})).await;
    let meta = json!({"browser":{"runtime":{"chromeHost":"127.0.0.1","chromePort":port}}});
    let err = session_endpoint(&meta, timeout, &cancel).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("Chrome has no advertised browser socket"),
        "{err:#}"
    );
    missing.abort();

    let (port, valid) = serve_json(
        json!({"webSocketDebuggerUrl":"ws://127.0.0.1:9222/devtools/browser/PUBLIC-advertised"}),
    )
    .await;
    let meta = json!({"browser":{"runtime":{"chromeHost":"127.0.0.1","chromePort":port}}});
    let endpoint = session_endpoint(&meta, timeout, &cancel)
        .await
        .unwrap()
        .expect("inspected endpoint");
    assert_eq!(endpoint.port, port);
    assert_eq!(
        endpoint.browser_ws_endpoint,
        format!("ws://127.0.0.1:{port}/devtools/browser/PUBLIC-advertised")
    );
    valid.abort();
}

#[test]
fn format_tabs_reports_signals_and_truncates_long_snippets() {
    let endpoint = AttachmentEndpoint {
        host: "PUBLIC-host".into(),
        port: 9222,
        browser_ws_endpoint: "ws://PUBLIC-host:9222/devtools/browser/PUBLIC".into(),
    };
    let long = format!("{}  {}", "a".repeat(117), "b".repeat(20));
    let exact = "c".repeat(120);
    let tabs = [
        json!({
            "targetId":"PUBLIC-target-1",
            "state":"running",
            "currentModelLabel":"PUBLIC-model",
            "assistantCount":2,
            "stopExists":true,
            "sendExists":false,
            "title":"PUBLIC title",
            "url":"https://chatgpt.com/c/public-b",
            "lastAssistantSnippet":long
        }),
        json!({"targetId":"PUBLIC-target-2","lastAssistantSnippet":exact}),
        json!({"targetId":"PUBLIC-target-3","stopExists":true,"sendExists":true}),
    ];
    let expected = format!(
        "Browser Tabs PUBLIC-host:9222\n\
         - PUBLIC-target-1 running model=PUBLIC-model turns=2 stop=yes send=no\n  title=PUBLIC title\n  url=https://chatgpt.com/c/public-b\n  last={}…\n\
         - PUBLIC-target-2 detached model=(unknown) turns=0 stop=no send=no\n  title=(untitled)\n  url=\n  last={}\n\
         - PUBLIC-target-3 detached model=(unknown) turns=0 stop=yes send=yes\n  title=(untitled)\n  url=\n",
        "a".repeat(117),
        "c".repeat(120)
    );
    assert_eq!(format_tabs(&endpoint, &tabs, &[]), expected);
}
