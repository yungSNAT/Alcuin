#![cfg(unix)]

use super::*;

/// Extracts the field name from a serde "missing field `name`" error message.
fn missing_field(message: &str) -> Option<String> {
    message
        .strip_prefix("missing field `")?
        .split('`')
        .next()
        .map(str::to_owned)
}

/// Collects JSON pointers of every object node, root first.
fn object_pointers(value: &Value, prefix: String, out: &mut Vec<String>) {
    if let Value::Object(map) = value {
        out.push(prefix.clone());
        for (key, nested) in map {
            object_pointers(nested, format!("{prefix}/{key}"), out);
        }
    }
}

/// Builds a structurally valid serialized `Link` by satisfying serde's
/// missing-field reports one at a time.
fn probe_link(parent_id: &str, child_id: &str) -> Value {
    let mut probe = json!({});
    for _ in 0..64 {
        let missing = match serde_json::from_value::<Link>(probe.clone()) {
            Ok(link) => return serde_json::to_value(link).unwrap(),
            Err(error) => missing_field(&error.to_string())
                .unwrap_or_else(|| panic!("Link probe stalled at {probe}: {error}")),
        };
        let mut candidates = vec![
            json!(parent_id),
            json!("chatgpt"),
            json!("grok"),
            json!("ChatGPT"),
            json!("Chatgpt"),
            json!(1),
            json!(true),
            json!({}),
            json!([]),
        ];
        if missing.to_lowercase().contains("child") {
            candidates.insert(0, json!(child_id));
        }
        let mut pointers = vec![];
        object_pointers(&probe, String::new(), &mut pointers);
        let mut chosen = None;
        let mut fallback = None;
        'search: for pointer in &pointers {
            for candidate in &candidates {
                let mut attempt = probe.clone();
                attempt
                    .pointer_mut(pointer)
                    .and_then(Value::as_object_mut)
                    .unwrap()
                    .insert(missing.clone(), candidate.clone());
                match serde_json::from_value::<Link>(attempt.clone()) {
                    Ok(_) => {
                        chosen = Some(attempt);
                        break 'search;
                    }
                    Err(error) => match missing_field(&error.to_string()) {
                        Some(next) if next != missing => {
                            chosen = Some(attempt);
                            break 'search;
                        }
                        Some(_) if fallback.is_none() => {
                            fallback = Some(attempt);
                        }
                        _ => {}
                    },
                }
            }
        }
        probe = chosen
            .or(fallback)
            .unwrap_or_else(|| panic!("No value satisfies Link field {missing} in {probe}"));
    }
    panic!("Link probe did not converge: {probe}");
}

#[test]
fn reserved_phase_requires_empty_artifact_array_before_child_capture() {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionStore::new(temp.path().join("sessions"));
    let parent_id = Uuid::new_v4().to_string();
    let child_id = Uuid::new_v4().to_string();
    let link = probe_link(&parent_id, &child_id);
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<Link>(link.clone()).unwrap()).unwrap(),
        link
    );

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let reserved_message = "Reserved consultation already has parent delivery or changed phase";
    let cancelled_message = "Attempt adoption was cancelled";
    let cases: [(&str, Option<Value>, &str); 5] = [
        ("reserved", Some(json!([])), cancelled_message),
        ("reserved", None, cancelled_message),
        (
            "reserved",
            Some(json!([{"kind": "file", "path": "PUBLIC-stale.txt"}])),
            reserved_message,
        ),
        ("reserved", Some(json!({"kind": "file"})), reserved_message),
        ("captured", Some(json!([])), reserved_message),
    ];
    for (phase, artifacts, expected) in cases {
        let mut parent = json!({
            "id": parent_id,
            "browser": {"consultation": {
                "active": link,
                "callbackIssued": false,
                "adoption": {
                    "version": 1,
                    "phase": phase,
                    "link": link,
                    "childEvidenceSha256": "PUBLIC-evidence",
                },
            }},
        });
        if let Some(artifacts) = artifacts.clone() {
            parent["artifacts"] = artifacts;
        }
        let child = json!({"id": child_id});
        let error = require_reserved(&store, &parent, &child, &cancelled, None).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(expected),
            "phase {phase}, artifacts {artifacts:?}: {message}"
        );
    }
}
