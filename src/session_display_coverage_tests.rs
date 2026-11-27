use super::*;
use serde_json::json;

#[test]
fn browser_model_names_fall_back_to_evidence_or_placeholders() {
    // No requested model and no session model: verified picker evidence applies.
    let unnamed = json!({"status":"completed","mode":"browser","browser":{"modelSelection":{"verified":true,"resolvedLabel":"Pro"}}});
    assert_eq!(browser_model_name(&unnamed, &Value::Null), "Pro");
    assert!(
        table_row(&unnamed, "public-unnamed").starts_with("completed Pro           browser "),
        "{}",
        table_row(&unnamed, "public-unnamed")
    );
    // A session model without a requested one: the evidence does not apply.
    let named = json!({"mode":"browser","model":"gpt-6-pro","browser":{"modelSelection":{"verified":true,"resolvedLabel":"Pro"}}});
    assert_eq!(browser_model_name(&named, &Value::Null), "n/a");
    // Same label in another case keeps the plain name without "(requested ...)".
    let same = json!({"mode":"browser","model":"gpt-6-pro","browser":{"modelSelection":{"verified":true,"resolvedLabel":"GPT-6-PRO"}}});
    assert_eq!(
        browser_model_with_key(&same, &json!("gpt-6-pro")),
        "GPT-6-PRO"
    );
}

#[test]
fn pricing_timestamps_and_owners_handle_unusual_records() {
    assert_eq!(
        priced_cost(&json!({"model":"PUBLIC-other","usage":{"inputTokens":5}})),
        None
    );
    assert_eq!(aligned_timestamp("PUBLIC not a time"), "Invalid Date");
    let owners = response_owners(&[
        json!({"response":{"responseId":"resp_PUBLIC_owned"}}),
        json!({"id":"public-owner","response":{"responseId":"resp_PUBLIC_owned"}}),
    ]);
    assert_eq!(owners.len(), 1);
    assert_eq!(
        owners.get("resp_PUBLIC_owned").map(String::as_str),
        Some("public-owner")
    );
}

#[test]
fn chain_lines_cover_self_follow_session_only_and_unowned_short_responses() {
    let cases = [
        (
            json!({"id":"public-self","options":{"followupSessionId":"public-self"}}),
            "root -> public-self",
        ),
        (
            json!({"id":"public-kid","options":{"followupSessionId":"public-mom"}}),
            "public-mom -> public-kid",
        ),
        (
            json!({"id":"public-x","options":{"previousResponseId":"resp_short"}}),
            "resp_short -> public-x",
        ),
    ];
    for (meta, expected) in cases {
        assert_eq!(chain_line(&meta, Vec::new), expected);
    }
}

#[test]
fn tree_rows_nest_grandchildren_and_survive_cycles() {
    let entries = vec![
        json!({"id":"r"}),
        json!({"id":"c1","options":{"followupSessionId":"r"}}),
        json!({"id":"g","options":{"followupSessionId":"c1"}}),
        json!({"id":"c2","options":{"followupSessionId":"r"}}),
    ];
    let rows = tree_rows(&entries, &BTreeMap::new());
    assert_eq!(rows.len(), 4, "{rows:?}");
    assert!(rows[0].ends_with("  r"), "{}", rows[0]);
    assert!(rows[1].ends_with("  ├─ c1"), "{}", rows[1]);
    assert!(rows[2].ends_with("  │  └─ g"), "{}", rows[2]);
    assert!(rows[3].ends_with("  └─ c2"), "{}", rows[3]);

    let cycle = vec![
        json!({"id":"a","options":{"followupSessionId":"b"}}),
        json!({"id":"b","options":{"followupSessionId":"a"}}),
    ];
    let rows = tree_rows(&cycle, &BTreeMap::new());
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].ends_with("  a"), "{}", rows[0]);
    assert!(rows[1].ends_with("  └─ b"), "{}", rows[1]);
}

#[test]
fn tree_rows_append_the_recovery_notice_for_dead_browser_workers() {
    let meta = json!({"id":"PUBLIC-run","status":"running","mode":"browser","lifecycle":{"workerPid":u32::MAX},"browser":{"runtime":{"chromePort":9222}}});
    let rows = tree_rows(std::slice::from_ref(&meta), &BTreeMap::new());
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].ends_with("  PUBLIC-run"), "{}", rows[0]);
    assert_eq!(
        rows[1],
        "Worker exited for PUBLIC-run; stored status remains running. Use \"alcuin session PUBLIC-run --harvest\" to inspect the saved conversation."
    );
}

#[test]
fn transport_reasons_map_to_reference_labels() {
    for (reason, expected) in [
        (
            "connection-lost",
            "connection-lost — connection lost before completion",
        ),
        ("client-abort", "client-abort — request aborted locally"),
        ("unknown", "unknown — unknown transport failure"),
        ("PUBLIC-odd", "PUBLIC-odd — transport error"),
    ] {
        assert_eq!(
            transport_summary(&json!({"reason": reason})).as_deref(),
            Some(expected)
        );
    }
    assert_eq!(transport_summary(&json!({"reason": ""})), None);
}

#[test]
fn attach_metadata_lists_single_models_and_artifact_validation_states() {
    let browser = json!({"id":"public-b","status":"completed","createdAt":12345,"mode":"browser","model":"gpt-6-pro",
        "browser":{"modelSelection":{"verified":true,"resolvedLabel":"GPT-6-PRO","status":7,"source":"picker","capturedAt":"t"}},
        "artifacts":[
            {"label":"PUBLIC-log","path":"PUBLIC/log.txt","validation":{"ok":false,"error":"PUBLIC-bad"}},
            {"kind":"zip","path":"PUBLIC/a.zip","validation":{"ok":false}},
            {"kind":"txt","path":"PUBLIC/b.txt","transfer":{"status":"done"}}
        ],
        "transport":{"reason":"connection-lost"}});
    assert_eq!(
        attach_metadata(&browser, "root -> public-b"),
        [
            "Chain: root -> public-b",
            "Created: 12345",
            "Status: completed",
            "Model: GPT-6-PRO",
            "Browser evidence:",
            "- model requestedKey=gpt-6-pro; target=(none); resolvedLabel=GPT-6-PRO; status=7; strategy=(default); verified=yes; source=picker; capturedAt=t",
            "Artifacts:",
            "- PUBLIC-log — PUBLIC/log.txt validation=PUBLIC-bad",
            "- zip — PUBLIC/a.zip validation=failed",
            "- txt — PUBLIC/b.txt transfer=done",
            "Transport: connection-lost — connection lost before completion",
        ]
    );
    let api = json!({"id":"public-api","status":"failed","createdAt":"PUBLIC-time","model":"PUBLIC-model"});
    assert_eq!(
        attach_metadata(&api, "root -> public-api"),
        [
            "Chain: root -> public-api",
            "Created: PUBLIC-time",
            "Status: failed",
            "Model: PUBLIC-model",
        ]
    );
}

#[test]
fn completion_summaries_mark_unknowns_browser_models_and_missing_slugs() {
    let browser = json!({"mode":"browser","model":"gpt-6-pro","elapsedMs":-1,"usage":{"inputTokens":0,"outputTokens":null}});
    assert_eq!(
        completion_summary(&browser).as_deref(),
        Some("unknown · gpt-6-pro[browser] · ↑0 ↓0* ↻0* Δ0*")
    );

    let Ok(huge) = serde_json::from_str::<Value>("1e400") else {
        return;
    };
    let mut meta = json!({"id":"public-inf","model":"PUBLIC-model","elapsedMs":0,
        "usage":{"inputTokens":0,"outputTokens":0,"reasoningTokens":0,"totalTokens":0}});
    meta["usage"]["cost"] = huge;
    let summary = completion_summary(&meta).unwrap();
    assert!(summary.ends_with(" | slug=public-inf"), "{summary}");
    match session_cost(&meta) {
        Some(cost) if !cost.is_finite() => {
            assert!(summary.contains(" · n/a · "), "{summary}");
        }
        _ => assert!(!summary.contains("n/a"), "{summary}"),
    }
}
