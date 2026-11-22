use super::*;
use serde_json::json;

#[test]
fn pricing_treats_unusable_token_counts_as_zero_but_respects_stored_null() {
    let (input_rate, _) = crate::providers::model_settings(policy::DEFAULT_API_MODEL, &Value::Null)
        .unwrap()
        .pricing
        .unwrap();
    let mut meta = json!({
        "model": policy::DEFAULT_API_MODEL,
        "usage": {"inputTokens": 31, "outputTokens": "PUBLIC-not-a-count"}
    });

    assert_eq!(priced_cost(&meta), Some(31.0 * input_rate));

    meta["usage"]["cost"] = Value::Null;
    assert_eq!(priced_cost(&meta), None);
    assert_eq!(session_cost(&meta), None);
}

#[test]
fn tree_rows_keep_ancestor_guides_through_multiple_recursive_siblings() {
    let entries = [
        json!({"id":"PUBLIC-grand-b","options":{"followupSessionId":"PUBLIC-child-a"}}),
        json!({"id":"PUBLIC-leaf","options":{"followupSessionId":"PUBLIC-grand-b"}}),
        json!({"id":"PUBLIC-child-a","options":{"followupSessionId":"PUBLIC-root"}}),
        json!({"id":"PUBLIC-grand-a","options":{"followupSessionId":"PUBLIC-child-a"}}),
        json!({"id":"PUBLIC-child-b","options":{"followupSessionId":"PUBLIC-root"}}),
        json!({"id":"PUBLIC-root"}),
    ];

    assert_eq!(
        tree_rows(&entries, &BTreeMap::new()),
        [
            table_row(&entries[5], "PUBLIC-root"),
            table_row(&entries[2], "├─ PUBLIC-child-a"),
            table_row(&entries[0], "│  ├─ PUBLIC-grand-b"),
            table_row(&entries[1], "│  │  └─ PUBLIC-leaf"),
            table_row(&entries[3], "│  └─ PUBLIC-grand-a"),
            table_row(&entries[4], "└─ PUBLIC-child-b"),
        ]
    );
}

#[test]
fn attach_metadata_omits_the_model_line_without_a_string_model_or_runs() {
    for meta in [
        json!({"status":"completed"}),
        json!({"status":"completed","model":null,"models":[]}),
        json!({"status":"completed","model":42,"models":[]}),
    ] {
        assert_eq!(
            attach_metadata(&meta, "root -> PUBLIC-session"),
            [
                "Chain: root -> PUBLIC-session",
                "Created: undefined",
                "Status: completed",
            ],
            "{meta}"
        );
    }
}

#[test]
fn display_log_reports_a_missing_session_but_skips_unmatched_runs() {
    const ID: &str = "public-log";
    const FILTER: &str = "PUBLIC-other-model";

    let temp = tempfile::tempdir().unwrap();
    let store = SessionStore::new(temp.path().to_path_buf());
    let single = json!({"model": policy::DEFAULT_API_MODEL, "models": []});
    let unnamed = json!({});
    let explicit_runs = json!({"models": [{"model": policy::DEFAULT_API_MODEL}]});

    let err = display_log(&store, ID, &single, Some(FILTER)).unwrap_err();
    assert_eq!(err.to_string(), r#"Session "public-log" not found."#);

    let combined = store.log(ID).map_err(|err| err.to_string());
    for (meta, filter) in [
        (&single, None),
        (&single, Some(" \t ")),
        (&unnamed, Some(FILTER)),
    ] {
        assert_eq!(
            display_log(&store, ID, meta, filter).map_err(|err| err.to_string()),
            combined,
            "filter: {filter:?}, metadata: {meta}"
        );
    }

    assert!(
        display_log(&store, ID, &explicit_runs, Some(FILTER))
            .unwrap()
            .is_empty(),
        "an unmatched run filter must return an empty log even if the session is missing"
    );
}
