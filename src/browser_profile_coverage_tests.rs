//! Coverage tests for native persistent-profile admission edge cases.

use super::*;

fn soon() -> std::time::Instant {
    std::time::Instant::now() + Duration::from_secs(2)
}

#[test]
fn attachment_marker_drop_keeps_markers_it_cannot_verify() {
    let home = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::open(home.path()).unwrap();
    let marker = registry.mark_attachment(soon()).unwrap();
    let path = registry.path().join(ATTACHMENTS);
    let stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let rows = stored["attachments"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], marker.id.as_str());

    let malformed = r#"{"version":2,"attachments":[]}"#;
    fs::write(&path, malformed).unwrap();
    drop(marker);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        malformed,
        "a failed marker removal must leave the record untouched"
    );
}

#[test]
fn attachment_marking_refuses_unverifiable_or_oversized_control_records() {
    // Directory in place of the record: cannot be verified as a control record.
    let home = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::open(home.path()).unwrap();
    fs::create_dir(registry.path().join(ATTACHMENTS)).unwrap();
    let error = registry
        .mark_attachment(soon())
        .err()
        .expect("a directory record must be refused");
    let text = format!("{error:#}");
    assert!(
        text.contains("Unable to verify profile control record; preserving ownership"),
        "{text}"
    );
    assert!(registry.path().join(ATTACHMENTS).is_dir());

    // Oversized record: refused before parsing and left as it was.
    let home = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::open(home.path()).unwrap();
    let path = registry.path().join(ATTACHMENTS);
    let oversized = vec![b' '; 64 * 1024 + 1];
    fs::write(&path, &oversized).unwrap();
    let error = registry
        .mark_attachment(soon())
        .err()
        .expect("an oversized record must be refused");
    let text = format!("{error:#}");
    assert!(
        text.contains("Profile control record exceeds its limit"),
        "{text}"
    );
    assert_eq!(fs::read(&path).unwrap(), oversized);
}

#[tokio::test]
async fn live_or_unknown_maintenance_owner_blocks_admission_until_timeout() {
    let current = ProcessIdentity::current().unwrap();
    let owners = [
        json!({"version":1,"processIdentity":current}),
        json!({"version":2,"kind":"PUBLIC-maintenance"}),
    ];
    for maintenance in owners {
        let home = tempfile::tempdir().unwrap();
        let registry = ProfileRegistry::open(home.path()).unwrap();
        atomic_json(
            &registry.path().join("oracle-tab-maintenance.json"),
            &maintenance,
        )
        .unwrap();
        let error = registry
            .acquire(
                "PUBLIC-maintenance",
                Duration::from_millis(300),
                &CancellationToken::new(),
            )
            .await
            .err()
            .expect("maintenance must block admission");
        let text = format!("{error:#}");
        assert!(
            text.contains("Timed out waiting for ChatGPT browser slot"),
            "{maintenance}: {text}"
        );
        assert!(
            registry.records().unwrap()["leases"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let waiters = registry
            .transaction(|_, waiters| Ok(waiters.clone()))
            .unwrap();
        assert!(
            waiters["waiters"].as_array().unwrap().is_empty(),
            "timed-out waiter must be retired: {waiters}"
        );
    }
}

#[test]
fn legacy_liveness_needs_a_valid_pid_and_a_recorded_start() {
    let mut exited = std::process::Command::new("true").spawn().unwrap();
    let dead_pid = exited.id();
    exited.wait().unwrap();
    assert!(ProcessIdentity::read(dead_pid).unwrap().is_none());
    let me = std::process::id();
    let cases = [
        (json!({}), Liveness::Unknown),
        (json!({"pid":0}), Liveness::Unknown),
        (json!({"pid":"PUBLIC-pid"}), Liveness::Unknown),
        (
            json!({"pid":dead_pid,"processStartedAtMs":1.0}),
            Liveness::Dead,
        ),
        (json!({"pid":me}), Liveness::Unknown),
        (
            json!({"pid":me,"processStartedAtMs":"PUBLIC-start"}),
            Liveness::Unknown,
        ),
    ];
    for (record, expected) in cases {
        assert!(record_liveness(&record) == expected, "{record}");
    }
}

#[tokio::test]
async fn loopback_probe_without_budget_proves_nothing() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    assert_eq!(
        list_endpoint_targets("127.0.0.1".into(), port, Duration::ZERO).await,
        EndpointTargets::Unknown
    );
    drop(listener);
    assert_eq!(
        list_endpoint_targets("127.0.0.1".into(), port, Duration::from_secs(1)).await,
        EndpointTargets::Gone
    );
}

#[tokio::test]
async fn reaper_keeps_records_of_a_live_owner_even_when_its_chrome_refuses() {
    let home = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::open(home.path()).unwrap();
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let owner = ProcessIdentity::current().unwrap();
    let id = Uuid::new_v4().to_string();
    let lease = json!({"id":id,"waiterId":Uuid::new_v4().to_string(),"pid":owner.pid,"processStartedAtMs":owner.started_at_ms,"processIdentity":owner,"sessionId":"public-live","chromeHost":"127.0.0.1","chromePort":port,"chromeTargetId":"PUBLIC-TARGET","targetDispositionPending":true,"capacityReleased":true,"promptSubmitted":true,"createdAt":"2000-01-01T00:00:00Z","updatedAt":"2000-01-01T00:00:00Z"});
    fs::write(
        registry.path().join(REGISTRY),
        serde_json::to_vec(&json!({"version":1,"leases":[lease]})).unwrap(),
    )
    .unwrap();
    let removed = registry
        .reap_absent_target_leases(true, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(removed, 0);
    let records = registry.records().unwrap();
    let leases = records["leases"].as_array().unwrap();
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0]["id"], id.as_str());
    assert_eq!(leases[0]["chromeTargetId"], "PUBLIC-TARGET");
}
