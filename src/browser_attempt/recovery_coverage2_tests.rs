use super::*;

fn recovery_store(
    (root, manifest): (tempfile::TempDir, Value),
) -> (tempfile::TempDir, SessionStore, Value) {
    let home = std::path::Path::new(manifest["home"].as_str().unwrap());
    (root, SessionStore::new(home.join(".oracle")), manifest)
}

#[tokio::test]
async fn delivery_of_an_unchanged_final_turn_only_rechecks_the_session() {
    // These remote fixtures have no completed browser turn, so a delivery cannot change the
    // final turn: sealing a changed turn is out of their reach (see recovery_coverage_tests).
    for kind in ["remote-selected", "remote-capture"] {
        let (_root, store, manifest) =
            recovery_store(crate::browser_runner::tests::remote_parent_attempt(kind));
        let parent_id = manifest["parentId"].as_str().unwrap();
        let child_id = manifest["childId"].as_str().unwrap();
        let cancel = CancellationToken::new();
        let guard = Guard::acquire(&store, parent_id, &RunnerLimits::default(), &cancel)
            .await
            .unwrap();
        let before = store.stored_for_update(child_id).unwrap();
        assert_eq!(
            crate::browser_turns::saved(&before).unwrap(),
            [] as [Value; 0],
            "{kind}"
        );
        let images = json!([{"path": "PUBLIC-delivery.png"}]);

        guard
            .seal_delivery(&before, &before, &images)
            .unwrap_or_else(|error| panic!("{kind}: {error:#}"));

        // No seal is installed and the attempt metadata is not written.
        assert!(guard.delivered_turn.get().is_none(), "{kind}");
        assert_eq!(store.stored_for_update(child_id).unwrap(), before, "{kind}");
    }
}
