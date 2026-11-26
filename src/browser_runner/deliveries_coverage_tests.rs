use super::*;

#[test]
fn turn_image_deliveries_rejects_unsafe_session_paths_even_with_no_saved_turns() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("sessions");
    std::fs::create_dir(&home).unwrap();
    let store = SessionStore::new(home);
    let id = Uuid::new_v4().to_string();
    let session_dir = store.directory(&id).unwrap();
    std::fs::create_dir_all(&session_dir).unwrap();
    let mut metadata = json!({
        "id": id,
        "cwd": temp.path(),
        "options": {},
        "browser": {"runtime": {"browserTurnIndex": 0, "completedBrowserTurns": []}}
    });

    assert!(
        turn_image_deliveries(&store, &metadata, 0)
            .unwrap()
            .is_empty()
    );
    assert!(!session_dir.join("artifacts").exists());

    let outside = temp.path().join("PUBLIC-outside");
    std::fs::create_dir(&outside).unwrap();
    for invalid_id in ["../PUBLIC-outside", outside.to_str().unwrap()] {
        let directory_error = store.directory(invalid_id).unwrap_err();
        metadata["id"] = json!(invalid_id);

        let Err(error) = turn_image_deliveries(&store, &metadata, 0) else {
            panic!("accepted unsafe session id {invalid_id:?} for an empty turn history");
        };
        assert_eq!(error.to_string(), directory_error.to_string());
    }

    assert!(std::fs::read_dir(outside).unwrap().next().is_none());
}
