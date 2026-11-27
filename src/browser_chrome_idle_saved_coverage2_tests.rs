use super::*;

#[tokio::test]
async fn saved_idle_ignores_non_object_receipts_but_rejects_malformed_owners() {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionStore::new(temp.path().join("sessions"));
    let profile = temp.path().join("profile");
    std::fs::create_dir(&profile).unwrap();
    let marker = profile.join("PUBLIC-profile-marker");
    std::fs::write(&marker, b"PUBLIC profile contents").unwrap();

    let created = store
        .create(
            json!({"prompt": "PUBLIC saved idle receipt", "model": "gpt-6-pro", "engine": "browser"}),
            &profile,
            None,
        )
        .unwrap();
    let session = created["id"].as_str().unwrap();

    for (receipt, expected_error) in [
        (None, None),
        (Some(Value::Null), None),
        (Some(json!(false)), None),
        (Some(json!("PUBLIC-not-an-owner")), None),
        (Some(json!(["PUBLIC-not-an-owner"])), None),
        (
            Some(json!({"version": "PUBLIC-invalid-version"})),
            Some("expected u8"),
        ),
    ] {
        if let Some(receipt) = &receipt {
            store
                .update(session, |current| {
                    let runtime = merge_object(
                        &current["browser"]["runtime"],
                        &json!({"persistentBrowserOwner": receipt}),
                    );
                    let browser = merge_object(&current["browser"], &json!({"runtime": runtime}));
                    Ok(Some(json!({"browser": browser})))
                })
                .unwrap();
        }

        let before = store.stored_for_update(session).unwrap();
        assert_eq!(
            &before["browser"]["runtime"]["persistentBrowserOwner"],
            receipt.as_ref().unwrap_or(&Value::Null),
            "receipt fixture: {receipt:?}"
        );

        let outcome = retire_saved_idle(&store, session, Duration::ZERO).await;
        if let Some(expected) = expected_error {
            let error = outcome.unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
        } else {
            assert_eq!(outcome.unwrap(), None, "receipt: {receipt:?}");
        }

        assert_eq!(store.stored_for_update(session).unwrap(), before);
        assert_eq!(std::fs::read(&marker).unwrap(), b"PUBLIC profile contents");
    }
}
