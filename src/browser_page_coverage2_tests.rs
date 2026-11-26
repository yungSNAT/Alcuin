use super::*;
use crate::cdp::fake::{Reply, peer};
use tokio::sync::{broadcast, mpsc, oneshot};

const IO_TIMEOUT: Duration = Duration::from_secs(2);

fn dialog_event(kind: &str, message: &str) -> Value {
    json!({
        "method": "Page.javascriptDialogOpening",
        "params": {"type": kind, "message": message}
    })
}

#[tokio::test]
async fn dialog_handler_recovers_from_lag_and_answers_after_a_rejected_reply() {
    tokio::time::timeout(Duration::from_secs(4), async {
        let mut backlog = vec![dialog_event("alert", "PUBLIC-overwritten")];
        backlog.extend((0..8_192).map(|_| json!({"method": "Network.dataReceived"})));
        backlog.extend([
            dialog_event("beforeunload", "PUBLIC-leave-page"),
            dialog_event("prompt", "PUBLIC-enter-text"),
        ]);

        let (prompt_answered, prompt_answer) = oneshot::channel();
        let mut prompt_answered = Some(prompt_answered);
        let (page, scripted_peer) =
            peer(IO_TIMEOUT, move |request| match request.method.as_str() {
                "Runtime.enable" => Reply::WithEvents(json!({}), std::mem::take(&mut backlog)),
                "Page.enable" => Reply::ok(),
                "Page.handleJavaScriptDialog" if request.params["accept"] == false => {
                    prompt_answered.take().unwrap().send(()).unwrap();
                    Reply::ok()
                }
                "Page.handleJavaScriptDialog" => Reply::Error("PUBLIC-answer-rejected".into()),
                "Runtime.disable" => Reply::Close,
                other => panic!("unexpected CDP command: {other}"),
            })
            .await;

        let cancel = CancellationToken::new();
        let (log_tx, mut logs) = mpsc::unbounded_channel();

        // Subscribe before the peer runs, but queue Runtime.enable first so the entire
        // burst arrives before Page.enable is acknowledged and the handler starts reading.
        let (enable, dismissal) = tokio::join!(
            biased;
            page.call("Runtime.enable", json!({}), IO_TIMEOUT, &cancel),
            dismiss_dialogs(&page, IO_TIMEOUT, &cancel, move |line| {
                log_tx.send(line.to_owned()).unwrap();
            }),
        );
        assert_eq!(enable.unwrap(), json!({}));
        let mut guard = dismissal.unwrap();

        assert_eq!(
            logs.recv().await.unwrap(),
            "[nav] accepting JS dialog (beforeunload): PUBLIC-leave-page"
        );
        let failure = logs.recv().await.unwrap();
        assert!(
            failure
                .strip_prefix("[nav] failed to answer JS dialog: ")
                .is_some_and(|detail| !detail.is_empty()),
            "{failure}"
        );
        assert_eq!(
            logs.recv().await.unwrap(),
            "[nav] dismissing JS dialog (prompt): PUBLIC-enter-text"
        );
        prompt_answer.await.unwrap();

        let mut observer = page.events();
        assert!(
            page.call("Runtime.disable", json!({}), IO_TIMEOUT, &cancel)
                .await
                .is_err()
        );
        let trace = scripted_peer.await.unwrap();
        let methods: Vec<_> = trace
            .iter()
            .map(|command| command["method"].as_str().unwrap())
            .collect();
        assert_eq!(
            methods,
            [
                "Runtime.enable",
                "Page.enable",
                "Page.handleJavaScriptDialog",
                "Page.handleJavaScriptDialog",
                "Runtime.disable",
            ]
        );
        assert_eq!(trace[2]["params"], json!({"accept": true}));
        assert_eq!(trace[3]["params"], json!({"accept": false}));

        // If the transport closes the event stream on disconnect, the handler must
        // finish on its own rather than waiting for the dismissal guard to abort it.
        if let Ok(Err(broadcast::error::RecvError::Closed)) =
            tokio::time::timeout(Duration::from_millis(100), observer.recv()).await
        {
            tokio::time::timeout(IO_TIMEOUT, &mut guard.0)
                .await
                .expect("dialog handler kept running after the event stream closed")
                .expect("dialog handler did not exit cleanly");
        }
        assert!(logs.try_recv().is_err(), "unexpected extra dialog log");
    })
    .await
    .expect("dialog backlog test did not finish");
}
