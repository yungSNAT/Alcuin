use super::*;

#[test]
fn read_candidate_propagates_the_error_for_missing_metadata() {
    let home = tempfile::tempdir().unwrap();
    let parent = home.path().to_owned();
    let path = parent.join("DevToolsActivePort");
    let directory = Arc::new(NativeEntry::open(&parent, true).unwrap());

    let Err(open_error) = directory.open_child(&parent, path.file_name().unwrap(), false) else {
        panic!("opened a metadata file that was never created");
    };
    let ancestry = vec![(parent, directory)];
    let Err(candidate_error) = read_candidate(path, &ancestry, "127.0.0.1") else {
        panic!("accepted missing browser metadata");
    };

    assert_eq!(format!("{candidate_error:#}"), format!("{open_error:#}"));
}

#[tokio::test]
async fn discovery_yields_at_the_entry_batch_boundary_and_observes_cancellation() {
    let home = tempfile::tempdir().unwrap();
    for index in 0..128 {
        fs::write(home.path().join(format!("PUBLIC-entry-{index:03}")), []).unwrap();
    }

    let roots = [home.path().to_owned()];
    let cancel = CancellationToken::new();
    let discovery = discover(
        &roots,
        "127.0.0.1",
        9222,
        Instant::now() + Duration::from_secs(2),
        &cancel,
    );
    tokio::pin!(discovery);

    assert!(
        futures_util::poll!(discovery.as_mut()).is_pending(),
        "discovery should yield after visiting 128 entries"
    );
    cancel.cancel();

    let Err(error) = discovery.await else {
        panic!("discovery ignored cancellation at the batch boundary");
    };
    assert_eq!(error.to_string(), "Running browser attachment cancelled");
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn failed_retry_reports_the_endpoint_and_both_metadata_search_roots() {
    use serde_json::json;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
    };

    let home = tempfile::tempdir().unwrap();
    let environment =
        Environment::from([("HOME".into(), home.path().to_string_lossy().into_owned())]);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = json!({"remoteChrome": {"host": "127.0.0.1", "port": port}});
    let cancel = CancellationToken::new();

    let serve_two_failures = async {
        for _ in 0..2 {
            let (socket, _) = listener.accept().await.unwrap();
            let mut request = BufReader::new(socket);
            let mut line = String::new();
            request.read_line(&mut line).await.unwrap();
            assert_eq!(line, "GET /json/version HTTP/1.1\r\n");

            loop {
                line.clear();
                assert_ne!(request.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }

            request
                .get_mut()
                .write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                )
                .await
                .unwrap();
        }
    };

    // Advance the retry timer without letting paused time jump straight to an
    // HTTP timeout while the kernel is still handling loopback I/O.
    let drive_clock = async {
        let started = Instant::now();
        let mut advanced = Duration::ZERO;
        loop {
            let elapsed = started.elapsed();
            if elapsed >= Duration::from_secs(3) {
                break;
            }
            let target = elapsed * 3;
            if target >= advanced + Duration::from_millis(5) {
                tokio::time::advance(target - advanced).await;
                advanced = target;
            } else {
                tokio::task::yield_now().await;
            }
        }
    };

    let (result, ()) = tokio::select! {
        outcome = async {
            tokio::join!(
                resolve(&config, &environment, Duration::from_secs(3), &cancel),
                serve_two_failures,
            )
        } => outcome,
        () = drive_clock => panic!("the two loopback probes did not finish"),
    };

    let Err(error) = result else {
        panic!("attached despite both endpoint probes returning HTTP 503");
    };
    assert_eq!(
        error.to_string(),
        format!(
            "No running browser matched 127.0.0.1:{port}; metadata searched {}, {}; endpoint http://127.0.0.1:{port}/json/version failed",
            home.path().join(".config").display(),
            home.path().join("snap").display(),
        ),
    );
    assert!(error.chain().nth(1).is_some(), "lost the probe error");
}
