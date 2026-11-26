//! Further coverage for the native remote host: the shutdown drain deadline that abandons
//! acknowledgement cleanup which outlasts it.

use super::*;

const TOKEN: &str = "PUBLIC coverage2 token";

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

/// A driver that records a running remote session and produces a plain answer.
fn driver() -> RunDriver {
    Arc::new(
        |context: RunContext| -> BoxFuture<'static, Result<ProducedRun>> {
            Box::pin(async move {
                let id = remote_result::session_id(&context.run_id)?;
                context.store.create(
                    json!({"prompt":"PUBLIC remote fixture"}),
                    context.store.home(),
                    Some(&id),
                )?;
                context.store.update(&id, |_| Ok(Some(json!({"status":"running","browser":{"remoteDelivery":{"runId":context.run_id}}}))))?;
                let result = json!({"answerText":"PUBLIC drain reply","answerMarkdown":"PUBLIC drain reply"});
                Ok(ProducedRun {
                    captured: result.clone(),
                    result,
                    artifacts: json!([]),
                })
            })
        },
    )
}

#[tokio::test]
async fn shutdown_abandons_acknowledgement_cleanup_that_outlasts_the_drain_deadline() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join(".oracle"));
    let host = RemoteHost::start(
        store.clone(),
        HostOptions {
            address: "127.0.0.1:0".parse().unwrap(),
            token: TOKEN.into(),
            max_concurrent_runs: Some(1),
            max_queued_runs: Some(1),
            max_browser_tabs: 1,
        },
        driver(),
    )
    .await
    .unwrap();
    let client = client();
    let url = format!("http://{}", host.address);

    let response = client
        .post(format!("{url}/runs"))
        .bearer_auth(TOKEN)
        .json(&json!({"prompt":"PUBLIC drain consultation"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let rows: Vec<Value> = response
        .text()
        .await
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(rows.iter().all(|row| row["type"] != "error"), "{rows:?}");
    let run = rows
        .iter()
        .find(|row| row["type"] == "run-started")
        .and_then(|row| row["runId"].as_str())
        .unwrap_or_else(|| panic!("no run-started row in {rows:?}"))
        .to_owned();

    let record = remote_result::read(&store, &run).unwrap();
    let delivery = json!({
        "version": 1,
        "runId": record.native()["runId"],
        "sha256": record.native()["delivery"]["sha256"],
    });

    // Hold the post-acknowledgement cleanup for this run indefinitely.
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let finished = Arc::new(AtomicBool::new(false));
    *remote_result::TEST_FINISH_GATE.lock().unwrap() = Some(remote_result::FinishGate {
        run: run.clone(),
        entered: entered.clone(),
        release: release.clone(),
        finished: finished.clone(),
    });

    let acknowledged = client
        .post(format!("{url}/runs/{run}/ack"))
        .bearer_auth(TOKEN)
        .json(&delivery)
        .send()
        .await
        .unwrap();
    assert_eq!(acknowledged.status(), 200);
    assert_eq!(
        acknowledged.json::<Value>().await.unwrap(),
        json!({"acknowledged":true})
    );
    let reached = tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .is_ok();
    drop(client);

    // With the clock paused, the five second drain deadline elapses as soon as the
    // runtime is idle, so the held cleanup worker is aborted rather than awaited.
    tokio::time::pause();
    let paused = tokio::time::Instant::now();
    let outcome = host.shutdown().await;
    let waited = paused.elapsed();
    {
        let mut gate = remote_result::TEST_FINISH_GATE.lock().unwrap();
        if gate.as_ref().is_some_and(|gate| gate.run == run) {
            *gate = None;
        }
    }

    assert!(
        reached,
        "the acknowledgement cleanup never reached its gate"
    );
    outcome.unwrap();
    assert!(
        waited >= Duration::from_secs(5),
        "shutdown did not wait for the drain deadline: {waited:?}"
    );
    assert!(
        !finished.load(Ordering::SeqCst),
        "the held cleanup should have been aborted, not completed"
    );
    assert_eq!(release.available_permits(), 0);
}
