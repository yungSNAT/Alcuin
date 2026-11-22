use super::*;
use std::{future::pending, pin::pin};
use tokio::{io::DuplexStream, time::timeout};
use tokio_tungstenite::tungstenite::protocol::Role;

const TEST_TIMEOUT: Duration = Duration::from_secs(1);
type TestSocket = WebSocketStream<DuplexStream>;

async fn socket_pair() -> (Cdp, TestSocket) {
    let (client, server) = tokio::io::duplex(4096);
    let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
    let server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    (Cdp::from_socket(client, TEST_TIMEOUT), server)
}

async fn read_request(server: &mut TestSocket) -> Value {
    let frame = timeout(TEST_TIMEOUT, server.next())
        .await
        .expect("peer did not receive a request")
        .expect("transport closed before sending a request")
        .expect("peer could not read the WebSocket frame");
    let Message::Text(text) = frame else {
        panic!("expected a CDP request, got {frame:?}");
    };
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn unsolicited_pong_does_not_interrupt_command_acknowledgement() {
    let (client, mut server) = socket_pair().await;
    let cancel = CancellationToken::new();
    let mut call = pin!(client.call("Runtime.enable", json!({}), TEST_TIMEOUT, &cancel));
    assert!(futures_util::poll!(call.as_mut()).is_pending());

    let request = read_request(&mut server).await;
    assert_eq!(request["method"], "Runtime.enable");

    let result = json!({"fixture": "PUBLIC-ack"});
    timeout(TEST_TIMEOUT, async {
        server
            .send(Message::Pong(bytes::Bytes::from_static(b"PUBLIC-pong")))
            .await
            .unwrap();
        server
            .send(Message::Text(
                json!({"id": request["id"], "result": result})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
    })
    .await
    .expect("peer could not send Pong and acknowledgement");

    assert_eq!(call.await.unwrap(), result);
    assert!(client.is_open());
}

#[tokio::test]
async fn disconnect_reports_queued_commands_as_unsent_and_written_command_as_uncertain() {
    let (client, mut server) = socket_pair().await;
    let cancel = CancellationToken::new();
    let mut written = pin!(client.call("Runtime.enable", json!({}), TEST_TIMEOUT, &cancel));
    assert!(futures_util::poll!(written.as_mut()).is_pending());
    assert_eq!(read_request(&mut server).await["method"], "Runtime.enable");

    // On this current-thread runtime, polling reserves queue entries without
    // giving the transport task a chance to dispatch them before shutdown.
    let mut queued = ["Page.enable", "DOM.enable"]
        .map(|method| Box::pin(client.call(method, json!({}), TEST_TIMEOUT, &cancel)));
    for call in &mut queued {
        assert!(futures_util::poll!(call.as_mut()).is_pending());
    }
    assert_eq!(
        client.connection.commands.capacity(),
        client.connection.commands.max_capacity() - queued.len()
    );

    client.disconnect_and_wait(TEST_TIMEOUT).await.unwrap();

    assert_eq!(
        written.await.unwrap_err().to_string(),
        "CDP endpoint disconnected; command acknowledgement is uncertain. Do not replay."
    );
    for call in queued {
        assert_eq!(
            call.await.unwrap_err().to_string(),
            "CDP connection is closed; command was not sent."
        );
    }

    let frame = timeout(TEST_TIMEOUT, server.next())
        .await
        .expect("peer did not observe the transport closing");
    assert!(
        !matches!(frame, Some(Ok(Message::Text(_)))),
        "a queued command reached the peer: {frame:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn cleanup_deadline_aborts_a_transport_that_never_finishes() {
    let (commands, _requests) = mpsc::channel(1);
    let (events, _) = broadcast::channel(1);
    // Model a stalled transport that never signals graceful completion.
    let mut client = Cdp {
        connection: Arc::new(Connection {
            commands,
            events,
            next: AtomicU64::new(1),
            shutdown: CancellationToken::new(),
            finished: CancellationToken::new(),
            task: tokio::spawn(pending::<()>()),
        }),
        session: None,
    };
    assert!(!client.is_closed());

    let error = client
        .disconnect_and_wait(Duration::from_millis(10))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "CDP transport cleanup exceeded its deadline and was aborted"
    );
    assert!(client.is_closed());

    let task = &mut Arc::get_mut(&mut client.connection).unwrap().task;
    let join_error = timeout(TEST_TIMEOUT, task)
        .await
        .expect("stalled transport task was not aborted")
        .unwrap_err();
    assert!(join_error.is_cancelled());
}
