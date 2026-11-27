use super::*;

#[test]
fn transcript_without_conversation_formats_artifact_validation() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let meta = store
        .create(json!({"prompt": "PUBLIC-prompt"}), home.path(), None)
        .unwrap();
    let id = meta["id"].as_str().unwrap();
    let current = json!({
        "artifacts": [
            {
                "kind": "image",
                "label": "PUBLIC-image",
                "path": "PUBLIC-image.png",
                "sha256": "PUBLIC-hash",
                "transfer": {"status": "not-needed"},
                "validation": {"ok": true, "error": "PUBLIC-ignored"}
            },
            {
                "kind": "file",
                "path": "PUBLIC-invalid.txt",
                "validation": {"ok": false, "error": "PUBLIC-invalid"}
            },
            {
                "kind": "file",
                "path": "PUBLIC-unverified.txt",
                "validation": {"ok": false}
            },
            {
                "kind": "file",
                "path": "PUBLIC-unchecked.txt",
                "validation": "PUBLIC-not-an-object"
            },
            {"kind": "output", "path": "PUBLIC-unlisted.md"}
        ]
    });

    let saved = save(
        &store,
        id,
        &current,
        " PUBLIC-prompt ",
        "\nPUBLIC-answer\n",
        &CancellationToken::new(),
    )
    .unwrap();

    assert_eq!(saved.len(), 1);
    let transcript = &saved[0];
    assert_eq!(transcript["kind"], "transcript");
    assert!(transcript.get("sourceUrl").is_none());

    let heading = crate::browser_output::answer_heading(&current);
    assert_eq!(
        fs::read_to_string(transcript["path"].as_str().unwrap()).unwrap(),
        format!(
            "# Oracle Browser Transcript\n\n\n\
             ## Prompt\n\nPUBLIC-prompt\n\n\
             ## {heading}\n\nPUBLIC-answer\n\n\
             ## Artifacts\n\n\
             - PUBLIC-image: PUBLIC-image.png sha256=PUBLIC-hash transfer=not-needed validation=ok\n\
             - file: PUBLIC-invalid.txt validation=PUBLIC-invalid\n\
             - file: PUBLIC-unverified.txt validation=failed\n\
             - file: PUBLIC-unchecked.txt\n"
        )
    );
}

#[test]
fn save_propagates_report_and_transcript_write_errors() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let meta = store
        .create(json!({"prompt": "PUBLIC-prompt"}), home.path(), None)
        .unwrap();
    let id = meta["id"].as_str().unwrap();
    let directory = store.directory(id).unwrap().join("artifacts");
    fs::write(&directory, "PUBLIC-blocker").unwrap();

    let answer = "PUBLIC-research report with enough words to keep.";
    let cancel = CancellationToken::new();
    for (current, kind, filename, label) in [
        (
            json!({"browser": {"config": {"researchMode": "deep"}}}),
            "deep-research-report",
            "deep-research-report.md",
            "Deep Research report",
        ),
        (
            json!({}),
            "transcript",
            "transcript.md",
            "Browser transcript",
        ),
    ] {
        // Compare with the writer's diagnostic without hard-coding OS error text.
        let expected = write(&directory, kind, filename, label, answer, None, &cancel).unwrap_err();
        let error = save(&store, id, &current, "PUBLIC-prompt", answer, &cancel).unwrap_err();

        assert_eq!(format!("{error:#}"), format!("{expected:#}"), "{kind}");
        assert_eq!(fs::read_to_string(&directory).unwrap(), "PUBLIC-blocker");
    }
}
