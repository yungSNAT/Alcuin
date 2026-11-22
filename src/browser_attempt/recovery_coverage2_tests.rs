use super::*;

fn recovery_store(
    (root, manifest): (tempfile::TempDir, Value),
) -> (tempfile::TempDir, SessionStore, Value) {
    let home = std::path::Path::new(manifest["home"].as_str().unwrap());
    (root, SessionStore::new(home.join(".oracle")), manifest)
}

fn write_metadata(store: &SessionStore, id: &str, meta: &Value) {
    crate::storage::atomic_json(&store.directory(id).unwrap().join("meta.json"), meta).unwrap();
}

#[tokio::test]
async fn delivery_seal_is_installed_once_and_only_for_a_verified_image_change() {
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
        let mut after = before.clone();
        let mut images = json!([{"path": "PUBLIC-delivery.png"}]);
        if let (Ok(index), Ok(rows)) = (
            crate::browser_turns::index(&before),
            crate::browser_turns::saved(&before),
        ) && index < rows.len()
        {
            // Change only the final row's images; prefer dropping images, which needs no
            // new artifact on disk.
            if rows[index]["images"] != json!([]) {
                images = json!([]);
            }
            if let Some(row) =
                after.pointer_mut(&format!("/browser/runtime/completedBrowserTurns/{index}"))
            {
                row["images"] = images.clone();
            }
        }
        write_metadata(&store, child_id, &after);
        let after = store.stored_for_update(child_id).unwrap();

        let result = guard.seal_delivery(&before, &after, &images);

        // Sealing a delivery never writes the attempt metadata.
        assert_eq!(store.stored_for_update(child_id).unwrap(), after, "{kind}");
        match result {
            Ok(()) if turn(&before) != turn(&after) => {
                assert_eq!(guard.delivered_turn.get(), Some(&turn(&after)), "{kind}");
                assert_eq!(guard.turn_seal(), &turn(&after), "{kind}");
                // A second delivery of the same final turn is refused once sealed.
                let error = guard.seal_delivery(&before, &after, &images).unwrap_err();
                assert_eq!(
                    error.to_string(),
                    "Recovery delivery differs from its admitted final turn",
                    "{kind}"
                );
                assert_eq!(guard.delivered_turn.get(), Some(&turn(&after)), "{kind}");
                assert_eq!(store.stored_for_update(child_id).unwrap(), after, "{kind}");
            }
            Ok(()) => {
                // An unchanged final turn only rechecks the session and installs no seal.
                assert!(guard.delivered_turn.get().is_none(), "{kind}");
            }
            Err(error) => {
                assert!(guard.delivered_turn.get().is_none(), "{kind}: {error:#}");
            }
        }
    }
}
