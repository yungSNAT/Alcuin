use super::*;

fn setup() -> (tempfile::TempDir, SessionStore, String) {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let meta = store
        .create(
            json!({"prompt":"PUBLIC coverage harvest","model":"gpt-6-astra"}),
            home.path(),
            None,
        )
        .unwrap();
    let id = meta["id"].as_str().unwrap().to_owned();
    (home, store, id)
}

fn saved(store: &SessionStore, id: &str, browser: Value, artifacts: Value) {
    store
        .update(id, |_| {
            Ok(Some(json!({"browser":browser,"artifacts":artifacts})))
        })
        .unwrap();
}

fn observed() -> Value {
    json!({"targetId":"PUBLIC-target","url":"https://chatgpt.com/c/public-a","lastAssistantText":"abc","state":"completed","stopExists":false,"sendExists":true,"assistantCount":1})
}

#[test]
fn required_urls_without_conversation_ids_are_reported_unverified() {
    let meta = json!({
        "browser": {
            "runtime": {"tabUrl": "https://example.com/PUBLIC-page"},
            "archive": {"conversationUrl": "https://chatgpt.com/c/public-a"}
        },
        "artifacts": [
            {"kind": "file", "sourceUrl": "https://example.com/PUBLIC-file"},
            {"kind": "transcript", "sourceUrl": "https://example.com/PUBLIC-transcript"},
            {"kind": "deep-research-report", "sourceUrl": "https://example.com/PUBLIC-report"}
        ]
    });
    let (captured, unverified) = metadata_evidence(&meta);
    assert_eq!(
        unverified,
        vec![
            "runtime-url".to_string(),
            "artifact:1".to_string(),
            "artifact:2".to_string()
        ]
    );
    assert_eq!(
        captured,
        vec![CapturedIdentity {
            source: "archive".into(),
            conversation_id: "public-a".into(),
        }]
    );
}

#[test]
fn normalise_handles_current_and_parent_components() {
    let cases: [(&str, Option<&str>); 5] = [
        ("./a/b", Some("a/b")),
        ("./a/../b", Some("b")),
        ("../x", None),
        ("a/../../c", None),
        ("/base/child/../file", Some("/base/file")),
    ];
    for (input, expected) in cases {
        assert_eq!(
            normalise(Path::new(input)),
            expected.map(PathBuf::from),
            "input {input}"
        );
    }
}

#[test]
fn transcripts_with_malformed_headers_are_unverified_not_captured() {
    let (_home, store, id) = setup();
    saved(
        &store,
        &id,
        json!({"runtime":{"conversationId":"public-a"}}),
        json!([]),
    );
    let dir = store.directory(&id).unwrap().join("artifacts");
    fs::create_dir(&dir).unwrap();
    let cases = [
        "# Not An Oracle Transcript\n\nConversation: https://chatgpt.com/c/public-a\n",
        "# Oracle Browser Transcript\nConversation: https://chatgpt.com/c/public-a\n",
        "",
    ];
    for contents in cases {
        fs::write(dir.join("transcript.md"), contents).unwrap();
        let integrity = persist(&store, &id, &observed(), false).unwrap();
        assert_eq!(integrity.status, "unverified", "contents {contents:?}");
        assert!(
            integrity
                .unverified_sources
                .contains(&"transcript:transcript.md".to_string()),
            "contents {contents:?}"
        );
        assert!(
            integrity
                .captured
                .iter()
                .all(|entry| entry.source != "transcript:transcript.md")
        );
        assert_eq!(
            store.require(&id).unwrap()["browser"]["harvest"]["integrity"]["status"],
            "unverified"
        );
    }
}

#[test]
fn harvest_without_observed_identity_drops_stale_conversation_id() {
    let (_home, store, id) = setup();
    saved(
        &store,
        &id,
        json!({"harvest":{"conversationId":"public-old","url":"https://chatgpt.com/c/public-old","extension":5}}),
        json!([]),
    );
    let harvested = json!({"lastAssistantText":"abc","state":"completed"});
    let integrity = persist(&store, &id, &harvested, false).unwrap();
    assert_eq!(integrity.status, "unverified");
    assert!(integrity.observed_conversation_id.is_none());
    assert!(integrity.captured.is_empty());
    assert_eq!(
        integrity.previous_harvest_conversation_id.as_deref(),
        Some("public-old")
    );
    let after = store.require(&id).unwrap();
    let harvest = &after["browser"]["harvest"];
    assert!(harvest.get("conversationId").is_none());
    assert!(harvest.get("url").is_none());
    assert_eq!(harvest["state"], "completed");
    assert_eq!(harvest["extension"], 5);
    assert!(harvest["integrity"].get("observedConversationId").is_none());
    assert_eq!(
        harvest["integrity"]["previousHarvestConversationId"],
        "public-old"
    );
    assert_eq!(
        harvest["assistantHash"],
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
}
