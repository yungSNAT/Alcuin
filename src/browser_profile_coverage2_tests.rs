//! Regression tests for admission delivery and exact waiter cleanup.

use super::*;

#[test]
fn abandoned_delivery_releases_unused_admission_but_preserves_target_authority() {
    let home = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::open(home.path()).unwrap();
    let cancel = CancellationToken::new();
    let unused = registry
        .acquire_blocking("PUBLIC-unused", 2, deadline_cleanup(), &cancel)
        .unwrap();
    let protected = registry
        .acquire_blocking("PUBLIC-protected", 2, deadline_cleanup(), &cancel)
        .unwrap();
    protected
        .update(&json!({"targetDispositionPending":true}))
        .unwrap();
    let before = registry.records().unwrap();
    assert_eq!(before["leases"][0]["id"], unused.id());
    assert_eq!(before["leases"][1]["id"], protected.id());
    assert_eq!(before["leases"][1]["targetDispositionPending"], true);

    // A single blocking worker lets a later job serve as a barrier for both Drops.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    for lease in [unused, protected] {
        drop(AdmissionDelivery {
            lease: Some(lease),
            runtime: runtime.handle().clone(),
        });
    }
    let (send, receive) = std::sync::mpsc::channel();
    drop(runtime.spawn_blocking(move || send.send(()).unwrap()));
    receive.recv_timeout(Duration::from_secs(2)).unwrap();

    assert!(
        !failed_releases()
            .iter()
            .any(|(path, _)| path == registry.path()),
        "a dropped delivery's release failed"
    );
    assert_eq!(
        registry.records().unwrap()["leases"],
        json!([before["leases"][1]])
    );
}

#[tokio::test]
async fn cancellation_after_promotion_reports_unsettled_cleanup_when_registry_is_unreadable() {
    tokio::task::spawn_blocking(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let home = tempfile::tempdir().unwrap();
            let registry = ProfileRegistry::open(home.path()).unwrap();
            let lock = DirectoryLock::acquire_until(
                &registry.path().join("oracle-tab-leases.lock"),
                deadline_cleanup(),
            )
            .unwrap();
            let cancel = CancellationToken::new();
            let mut admission = std::pin::pin!(registry.acquire_with_limit(
                "PUBLIC-undelivered",
                1,
                Duration::from_secs(2),
                &cancel,
            ));

            // Start the worker while the lock prevents promotion. Then let it finish
            // without polling the future that would deliver the lease to its caller.
            assert!(futures_util::poll!(admission.as_mut()).is_pending());
            lock.release().unwrap();
            tokio::task::spawn_blocking(|| {}).await.unwrap();

            let path = registry.path().join(REGISTRY);
            let persisted: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let leases = persisted["leases"].as_array().unwrap();
            assert_eq!(leases.len(), 1);
            assert_eq!(leases[0]["sessionId"], "PUBLIC-undelivered");

            let malformed = "{PUBLIC-incomplete";
            fs::write(&path, malformed).unwrap();
            cancel.cancel();
            let error = admission
                .await
                .err()
                .expect("a cancelled delivery must not grant admission");
            failed_releases().retain(|(path, _)| path != registry.path());

            assert!(
                error.downcast_ref::<UnsettledAdmission>().is_some(),
                "{error:#}"
            );
            assert!(
                error.to_string().contains(
                    "Browser admission was cancelled; exact admission cleanup remains pending"
                ),
                "{error:#}"
            );
            assert_eq!(fs::read_to_string(path).unwrap(), malformed);
        });
    })
    .await
    .unwrap();
}

async fn queue_containing(path: &Path, session: &str, deadline: Instant) -> Vec<Value> {
    tokio::time::timeout_at(deadline, async {
        loop {
            // Read the atomically replaced file without competing for the admission lock.
            if let Ok(bytes) = fs::read(path)
                && let Ok(queue) = serde_json::from_slice::<Value>(&bytes)
                && let Some(rows) = queue["waiters"].as_array()
                && rows.iter().any(|row| row["sessionId"] == session)
            {
                return rows.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("waiter for {session} did not appear"))
}

#[tokio::test]
async fn reclaimed_waiter_rejoins_at_the_tail_with_a_new_token() {
    const SESSION: &str = "PUBLIC-requeued";

    let home = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::open(home.path()).unwrap();
    atomic_json(
        &registry.path().join("oracle-tab-maintenance.json"),
        &json!({"version":1,"processIdentity":ProcessIdentity::current().unwrap()}),
    )
    .unwrap();

    let cancel = CancellationToken::new();
    let acquiring = registry.clone();
    let worker_cancel = cancel.clone();
    let deadline = Instant::now() + Duration::from_secs(2);
    let task = tokio::spawn(async move {
        acquiring
            .acquire_with_limit(SESSION, 1, Duration::from_secs(3), &worker_cancel)
            .await
    });
    let path = registry.path().join(WAITERS);
    let original = queue_containing(&path, SESSION, deadline).await;
    assert_eq!(original.len(), 1);

    let mut front = original[0].clone();
    front["id"] = json!(Uuid::new_v4().to_string());
    front["sessionId"] = json!("PUBLIC-front");
    registry
        .transaction_until(Some(deadline_cleanup()), |_, waiters| {
            let rows = waiters["waiters"].as_array_mut().unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["id"], original[0]["id"]);
            rows[0] = front.clone();
            Ok(())
        })
        .unwrap();

    let requeued = queue_containing(&path, SESSION, deadline).await;
    cancel.cancel();
    let error = task
        .await
        .unwrap()
        .err()
        .expect("cancellation must stop the requeued admission");
    assert!(error.to_string().contains("cancelled"), "{error:#}");

    assert_eq!(requeued.len(), 2);
    assert_eq!(requeued[0], front);
    assert_eq!(requeued[1]["sessionId"], SESSION);
    assert!(uuid_shaped(&requeued[1]["id"]));
    assert_ne!(requeued[1]["id"], original[0]["id"]);
    assert_eq!(requeued[1]["createdAt"], original[0]["createdAt"]);
    registry
        .transaction_until(Some(deadline_cleanup()), |records, waiters| {
            assert!(records["leases"].as_array().unwrap().is_empty());
            assert_eq!(waiters["waiters"], json!([front]));
            Ok(())
        })
        .unwrap();
}

#[test]
fn retrying_failed_release_removes_only_the_exact_waiter() {
    let owner = ProcessIdentity::current().unwrap();
    for (patch, exact) in [
        (json!({}), true),
        (json!({"id":Uuid::new_v4().to_string()}), false),
        (json!({"sessionId":"PUBLIC-replacement"}), false),
        (json!({"createdAt":"2000-01-01T00:00:01Z"}), false),
    ] {
        let home = tempfile::tempdir().unwrap();
        let at = "2000-01-01T00:00:00Z";
        let lease = json!({
            "id": Uuid::new_v4().to_string(),
            "waiterId": Uuid::new_v4().to_string(),
            "pid": owner.pid,
            "processStartedAtMs": owner.started_at_ms,
            "processIdentity": owner,
            "sessionId": "PUBLIC-retained",
            "createdAt": at,
            "updatedAt": at
        });
        let waiter = merge_object(
            &json!({
                "id": lease["waiterId"],
                "pid": owner.pid,
                "processStartedAtMs": owner.started_at_ms,
                "processIdentity": owner,
                "sessionId": lease["sessionId"],
                "maxConcurrentTabs": 1,
                "createdAt": at,
                "updatedAt": at
            }),
            &patch,
        );
        let mut records = json!({"version":1,"leases":[lease]});
        let mut waiters = json!({"version":1,"waiters":[waiter]});
        retain_failed_release(
            home.path(),
            &lease,
            &anyhow::anyhow!("PUBLIC-cleanup-failure"),
        );
        let settled = retry_failed_releases(home.path(), &mut records, &mut waiters);
        forget_settled_releases(home.path(), &settled);

        assert_eq!(settled.len(), 1, "{patch}");
        assert_eq!(settled[0]["id"], lease["id"]);
        assert!(records["leases"].as_array().unwrap().is_empty());
        let expected = if exact { json!([]) } else { json!([waiter]) };
        assert_eq!(waiters["waiters"], expected, "{patch}");
        assert!(
            !failed_releases()
                .iter()
                .any(|(path, _)| path == home.path())
        );
    }
}
