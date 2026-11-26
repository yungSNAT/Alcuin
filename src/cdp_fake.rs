//! A scripted CDP peer for unit tests: a closure answers each command the code under test sends.
//!
//! No matter where you go, everyone's connected.

use super::Cdp;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

/// One command the code under test sent.
pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) params: Value,
    pub(crate) session: Option<String>,
}

impl Request {
    /// The `Runtime.evaluate` expression, or "" for other methods.
    pub(crate) fn expression(&self) -> &str {
        self.params["expression"].as_str().unwrap_or_default()
    }
    /// The `alcuin-dom:<name>` marker inside an evaluated browser script.
    pub(crate) fn dom(&self) -> Option<&str> {
        let rest = self.expression().split_once("alcuin-dom:")?.1;
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(rest.len());
        Some(&rest[..end])
    }
    /// The first argument passed through `Cdp::evaluate_with_arguments`, or null.
    pub(crate) fn arg(&self) -> Value {
        self.expression()
            .rsplit_once(")(...")
            .and_then(|(_, tail)| tail.strip_suffix(')'))
            .and_then(|args| serde_json::from_str::<Value>(args).ok())
            .map_or(Value::Null, |args| args[0].clone())
    }
}

/// The peer's answer to one command.
pub(crate) enum Reply {
    /// Send `{"result": <value>}`.
    Result(Value),
    /// Send `{"result": <value>}`, then each event.
    WithEvents(Value, Vec<Value>),
    /// Send `{"error": {"message": <text>}}`.
    Error(String),
    /// Send nothing, so the command times out.
    Silent,
    /// Close the socket.
    Close,
}

impl Reply {
    /// An empty successful result.
    pub(crate) fn ok() -> Self {
        Self::Result(json!({}))
    }
    /// A `Runtime.evaluate` result whose value is `value`.
    pub(crate) fn value(value: Value) -> Self {
        Self::Result(json!({"result": {"value": value}}))
    }
    /// A `Runtime.evaluate` result whose page script threw `description`.
    pub(crate) fn thrown(description: &str) -> Self {
        Self::Result(json!({
            "result": {"type": "object", "subtype": "error"},
            "exceptionDetails": {"text": "Uncaught", "exception": {"description": description}}
        }))
    }
}

/// A `Cdp` connected to a scripted peer. The handle yields every command received, as sent.
pub(crate) async fn peer<F>(
    io_timeout: Duration,
    mut respond: F,
) -> (Cdp, tokio::task::JoinHandle<Vec<Value>>)
where
    F: FnMut(&Request) -> Reply + Send + 'static,
{
    let (client, server) = tokio::io::duplex(1024 * 1024);
    let cdp = Cdp::from_socket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
        io_timeout,
    );
    let mut socket = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    let task = tokio::spawn(async move {
        let mut trace = vec![];
        while let Some(Ok(Message::Text(frame))) = socket.next().await {
            let message: Value = serde_json::from_str(&frame).unwrap();
            let request = Request {
                method: message["method"].as_str().unwrap_or_default().to_owned(),
                params: message["params"].clone(),
                session: message["sessionId"].as_str().map(str::to_owned),
            };
            let reply = respond(&request);
            let id = message["id"].clone();
            trace.push(message);
            let (frame, events) = match reply {
                Reply::Result(result) => (json!({"id": id, "result": result}), vec![]),
                Reply::WithEvents(result, events) => (json!({"id": id, "result": result}), events),
                Reply::Error(text) => (json!({"id": id, "error": {"message": text}}), vec![]),
                Reply::Silent => continue,
                Reply::Close => break,
            };
            for value in std::iter::once(frame).chain(events) {
                if socket
                    .send(Message::Text(value.to_string().into()))
                    .await
                    .is_err()
                {
                    return trace;
                }
            }
        }
        trace
    });
    (cdp, task)
}

/// A loopback DevTools endpoint: HTTP discovery plus WebSocket sessions on any path.
pub(crate) struct Browser {
    pub(crate) port: u16,
    /// Every command received on any socket, with its socket's path under `"path"`.
    pub(crate) trace: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Browser {
    /// `http://127.0.0.1:<port>`.
    pub(crate) fn http(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    /// `ws://127.0.0.1:<port>/devtools/browser/PUBLIC`, which `/json/version` advertises.
    pub(crate) fn browser_ws(&self) -> String {
        format!("ws://127.0.0.1:{}/devtools/browser/PUBLIC", self.port)
    }
    /// The methods received so far, in order.
    pub(crate) fn methods(&self) -> Vec<String> {
        let trace = self.trace.lock().unwrap();
        trace
            .iter()
            .map(|m| m["method"].as_str().unwrap_or_default().to_owned())
            .collect()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Serve `/json/version`, `/json/list` (answering `targets`) and WebSocket sessions whose commands
/// `respond` answers; `Request::session` carries the socket path when the command has no session.
pub(crate) async fn browser<F>(targets: Value, respond: F) -> Browser
where
    F: FnMut(&Request) -> Reply + Send + 'static,
{
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let respond = Arc::new(Mutex::new(respond));
    let trace = Arc::new(Mutex::new(vec![]));
    let recorded = trace.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut head = [0; 2048];
            let peeked = stream.peek(&mut head).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&head[..peeked]).to_ascii_lowercase();
            if !head.contains("upgrade: websocket") {
                let mut request = vec![0; 2048];
                let read = stream.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]).into_owned();
                let path = request.split(' ').nth(1).unwrap_or_default();
                let (status, body) = match path {
                    "/json/version" => (
                        "200 OK",
                        json!({"Browser": "PUBLIC", "webSocketDebuggerUrl": format!("ws://127.0.0.1:{port}/devtools/browser/PUBLIC")}),
                    ),
                    "/json/list" | "/json" => ("200 OK", targets.clone()),
                    _ => ("404 Not Found", json!({})),
                };
                let body = body.to_string();
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
                continue;
            }
            let respond = respond.clone();
            let trace = recorded.clone();
            tokio::spawn(async move {
                let mut path = String::new();
                #[expect(
                    clippy::result_large_err,
                    reason = "Tungstenite requires its unboxed ErrorResponse in the handshake callback"
                )]
                let capture =
                    |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                     response| {
                        path = request.uri().path().to_owned();
                        Ok(response)
                    };
                let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(stream, capture).await
                else {
                    return;
                };
                while let Some(Ok(Message::Text(frame))) = socket.next().await {
                    let mut message: Value = serde_json::from_str(&frame).unwrap();
                    let request = Request {
                        method: message["method"].as_str().unwrap_or_default().to_owned(),
                        params: message["params"].clone(),
                        session: message["sessionId"]
                            .as_str()
                            .map_or_else(|| Some(path.clone()), |s| Some(s.to_owned())),
                    };
                    let reply = {
                        let mut respond = respond.lock().unwrap();
                        (*respond)(&request)
                    };
                    let id = message["id"].clone();
                    message["path"] = json!(path);
                    trace.lock().unwrap().push(message);
                    let (frame, events) = match reply {
                        Reply::Result(result) => (json!({"id": id, "result": result}), vec![]),
                        Reply::WithEvents(result, events) => {
                            (json!({"id": id, "result": result}), events)
                        }
                        Reply::Error(text) => {
                            (json!({"id": id, "error": {"message": text}}), vec![])
                        }
                        Reply::Silent => continue,
                        Reply::Close => break,
                    };
                    for value in std::iter::once(frame).chain(events) {
                        if socket
                            .send(Message::Text(value.to_string().into()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            });
        }
    });
    Browser {
        port,
        trace,
        server,
    }
}

#[tokio::test]
async fn fake_browser_serves_discovery_and_sockets() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let fake = browser(json!([{"id": "PUBLIC-page", "type": "page"}]), |request| {
        Reply::Result(json!({"path": request.session}))
    })
    .await;
    let short = Duration::from_secs(1);
    let version = crate::cdp::endpoint_json(&fake.http(), "/json/version", short, &cancel)
        .await
        .unwrap();
    assert_eq!(version["webSocketDebuggerUrl"], fake.browser_ws());
    let list = crate::cdp::endpoint_json(&fake.http(), "/json/list", short, &cancel)
        .await
        .unwrap();
    assert_eq!(list[0]["id"], "PUBLIC-page");
    let cdp = Cdp::connect(&fake.browser_ws(), short, &cancel)
        .await
        .unwrap();
    let answer = cdp
        .call("Target.getTargets", json!({}), short, &cancel)
        .await
        .unwrap();
    assert_eq!(answer["path"], "/devtools/browser/PUBLIC");
    assert_eq!(fake.methods(), ["Target.getTargets"]);
}

#[tokio::test]
async fn scripted_peer_answers_results_errors_events_and_throws() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let (cdp, peer) = peer(Duration::from_secs(1), |request| {
        match request.method.as_str() {
            "Page.enable" => {
                Reply::WithEvents(json!({}), vec![json!({"method": "Page.loadEventFired"})])
            }
            "Runtime.evaluate" if request.dom() == Some("probe") => {
                Reply::value(json!({"seen": request.arg()}))
            }
            "Runtime.evaluate" => Reply::thrown("TypeError: fixture"),
            "Runtime.enable" => Reply::ok(),
            "Network.enable" => Reply::Silent,
            "Page.close" => Reply::Close,
            _ => Reply::Error("unknown method".into()),
        }
    })
    .await;
    let mut events = cdp.events();
    let short = Duration::from_millis(200);
    cdp.call("Page.enable", json!({}), short, &cancel)
        .await
        .unwrap();
    assert_eq!(
        events.recv().await.unwrap()["method"],
        "Page.loadEventFired"
    );
    let seen = cdp
        .evaluate_with_arguments(
            "/* alcuin-dom:probe */ (cfg)=>cfg",
            &json!([{"n": 1}]),
            short,
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(seen, json!({"seen": {"n": 1}}));
    assert_eq!(
        cdp.call("Runtime.enable", json!({}), short, &cancel)
            .await
            .unwrap(),
        json!({})
    );
    let thrown = cdp.evaluate("1", short, &cancel).await.unwrap_err();
    assert!(
        thrown.to_string().contains("TypeError: fixture"),
        "{thrown:#}"
    );
    assert!(
        cdp.call("DOM.enable", json!({}), short, &cancel)
            .await
            .is_err()
    );
    assert!(
        cdp.call("Network.enable", json!({}), short, &cancel)
            .await
            .is_err()
    );
    assert!(
        cdp.call("Page.close", json!({}), short, &cancel)
            .await
            .is_err()
    );
    let trace = peer.await.unwrap();
    let methods: Vec<_> = trace
        .iter()
        .map(|m| m["method"].as_str().unwrap())
        .collect();
    assert_eq!(
        methods,
        [
            "Page.enable",
            "Runtime.evaluate",
            "Runtime.enable",
            "Runtime.evaluate",
            "DOM.enable",
            "Network.enable",
            "Page.close"
        ]
    );
}
