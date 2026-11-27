use super::*;

#[test]
fn selected_retirement_requires_confirmed_absence_and_propagates_settlement_errors() {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionStore::new(temp.path().to_path_buf());
    let journal = TargetJournal::open(&store).unwrap();
    let cancel = CancellationToken::new();
    let parent_id = "PUBLIC-missing-parent";
    let creator = json!({});

    for (disposition, reason) in [
        (
            json!({
                "decision": "retired",
                "absence": "unconfirmed",
                "reason": "PUBLIC-target-still-present"
            }),
            "PUBLIC-target-still-present",
        ),
        (
            json!({"decision": "retained", "absence": "confirmed-absent"}),
            "No cleanup reason recorded",
        ),
    ] {
        let error = finish_retirement(&store, parent_id, &journal, &creator, &disposition, &cancel)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Selected child target absence remains unconfirmed: {reason}")
        );
    }

    let disposition = json!({"decision": "retired", "absence": "confirmed-absent"});
    let expected = crate::browser_runner::settle_completed_retirement(
        &store,
        &journal,
        &creator,
        &disposition,
        Instant::now() + Duration::from_secs(2),
    )
    .expect_err("confirmed absence alone cannot settle a retirement without a creator");

    let error = finish_retirement(&store, parent_id, &journal, &creator, &disposition, &cancel)
        .unwrap_err();
    // A later parent lookup would also fail; it must not mask the settlement error.
    assert_eq!(format!("{error:#}"), format!("{expected:#}"));
}
