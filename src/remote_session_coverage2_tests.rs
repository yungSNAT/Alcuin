use super::*;
use crate::browser_prompt::{BundleFormat, BundleScope, PendingBundle};
use std::fs;

const PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe, 0x00, 0x01,
];

fn offline_connection() -> Connection {
    Connection {
        host: "127.0.0.1:1".into(),
        token: None,
    }
}

fn browser_request(cwd: &Path) -> ResolvedRequest {
    let mut request = crate::execution::resolve(
        &json!({"prompt":"PUBLIC-receiver-prompt","engine":"browser","model":"gpt-6-pro"}),
        &json!({}),
        &Environment::new(),
        cwd,
    )
    .unwrap();
    request.options["browserConfig"] = request.browser.clone();
    request
}

fn start_session(
    store: &SessionStore,
    cwd: &Path,
    request: &ResolvedRequest,
    connection: &Connection,
) -> (String, ProcessIdentity) {
    let metadata = prepare(store, request, cwd, connection).unwrap();
    let id = metadata["id"].as_str().unwrap().to_owned();
    let identity = ProcessIdentity::current().unwrap();
    crate::execution::start_owner(store, &id, &identity, false).unwrap();
    (id, identity)
}

fn install_fallback(
    store: &SessionStore,
    id: &str,
    pending: PendingBundle,
    uploads: &[Value],
) -> StagedUploads {
    let staged = StagedUploads::capture_remote(store, id, uploads).unwrap();
    let mut committed = browser_runner::request(store, id).unwrap();
    committed.fallback = Some(browser_runner::BrowserFallback {
        composer_text: "PUBLIC-fallback-prompt".into(),
        pending_bundle: Some(pending),
        text_sources: vec![],
        raw_source: None,
        remote_source: Some(staged.clone()),
    });
    crate::storage::atomic_json(
        &store.directory(id).unwrap().join("browser-request.json"),
        &committed,
    )
    .unwrap();
    staged
}

fn wire_file(name: &str, bytes: &[u8]) -> Value {
    json!({
        "fileName": name,
        "displayPath": format!("PUBLIC-inputs/{name}"),
        "contentBase64": STANDARD.encode(bytes),
    })
}

fn persisted_text(root: &Path) -> String {
    let mut text = String::new();
    let Ok(entries) = fs::read_dir(root) else {
        return text;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            text.push_str(&persisted_text(&path));
        } else if let Ok(bytes) = fs::read(&path) {
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    text
}

fn session_entries(store: &SessionStore, id: &str) -> Vec<std::ffi::OsString> {
    let mut entries = fs::read_dir(store.directory(id).unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

/// Finds the non-ZIP bundle format through the committed serde representation.
fn text_bundle() -> PendingBundle {
    let zip = serde_json::to_value(PendingBundle {
        format: BundleFormat::Zip,
        scope: BundleScope::All,
    })
    .unwrap();
    let capitalized = |name: &str| {
        let mut chars = name.chars();
        chars
            .next()
            .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
            .unwrap_or_default()
    };
    [
        "text", "txt", "markdown", "md", "plain", "inline", "paste", "files",
    ]
    .into_iter()
    .flat_map(|name| {
        [
            name.to_owned(),
            capitalized(name),
            name.to_ascii_uppercase(),
        ]
    })
    .find_map(|name| {
        let mut candidate = zip.clone();
        candidate["format"] = json!(name);
        serde_json::from_value::<PendingBundle>(candidate)
            .ok()
            .filter(|bundle| bundle.format != BundleFormat::Zip)
    })
    .expect("PendingBundle exposes a non-ZIP format")
}

fn decoded_attachments(submission: &Value) -> Vec<Vec<u8>> {
    let attachments = submission["attachments"].as_array().unwrap();
    attachments
        .iter()
        .map(|attachment| {
            let bytes = STANDARD
                .decode(attachment["contentBase64"].as_str().unwrap())
                .unwrap();
            assert_eq!(attachment["sizeBytes"], json!(bytes.len()), "{submission}");
            bytes
        })
        .collect()
}

#[test]
fn preparation_strips_credentials_when_options_or_browser_config_are_not_objects() {
    const SECRET: &str = "PUBLIC-browser-token-fixture";
    for shape in ["missing-browser-config", "null-options"] {
        let home = tempfile::tempdir().unwrap();
        let cwd = home.path();
        let root = cwd.join("receiver");
        let store = SessionStore::new(root.clone());
        let mut request = browser_request(cwd);
        request.browser["remoteToken"] = json!(SECRET);
        request.browser["inlineCookies"] = json!(SECRET);
        match shape {
            "missing-browser-config" => {
                request
                    .options
                    .as_object_mut()
                    .unwrap()
                    .remove("browserConfig");
            }
            _ => request.options = Value::Null,
        }
        let outcome = prepare(&store, &request, cwd, &offline_connection());
        if shape == "missing-browser-config" {
            let meta = outcome.unwrap();
            let saved = store.require(meta["id"].as_str().unwrap()).unwrap();
            assert_eq!(saved["status"], "pending");
        }
        assert!(
            !persisted_text(&root).contains(SECRET),
            "{shape} persisted an ephemeral credential"
        );
    }
}

#[test]
fn text_bundle_fallback_materialises_text_sections_and_skips_raw_contents() {
    let bundle = text_bundle();

    // Text and raw sources: the text bundle carries the text file sections.
    {
        let home = tempfile::tempdir().unwrap();
        let cwd = home.path();
        let store = SessionStore::new(cwd.join("receiver"));
        let (id, _) = start_session(&store, cwd, &browser_request(cwd), &offline_connection());
        let source = install_fallback(
            &store,
            &id,
            bundle,
            &[
                wire_file("PUBLIC-notes.txt", b"PUBLIC-text-body"),
                wire_file("PUBLIC-image.png", PNG),
            ],
        );
        let before = session_entries(&store, &id);

        let mut outgoing = json!({});
        materialise_fallback(&store, &id, &mut outgoing).unwrap();
        let submission = &outgoing["fallbackSubmission"];
        assert_eq!(submission["prompt"], "PUBLIC-fallback-prompt");
        let decoded = decoded_attachments(submission);
        assert!(!decoded.is_empty(), "{submission}");
        let bundle_text = decoded
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .find(|text| text.contains("PUBLIC-text-body"))
            .unwrap_or_else(|| panic!("text bundle missing from {submission}"));
        assert!(bundle_text.contains("PUBLIC-inputs/PUBLIC-notes.txt"));
        assert!(bundle_text.ends_with('\n'));
        assert!(!bundle_text.contains("PUBLIC-image.png"), "{bundle_text}");
        assert_eq!(session_entries(&store, &id), before);
        source.verify(&store, &id).unwrap();
    }

    // Raw-only sources: no text sections are produced for a bundle.
    {
        let home = tempfile::tempdir().unwrap();
        let cwd = home.path();
        let store = SessionStore::new(cwd.join("receiver"));
        let (id, _) = start_session(&store, cwd, &browser_request(cwd), &offline_connection());
        let source = install_fallback(&store, &id, bundle, &[wire_file("PUBLIC-image.png", PNG)]);
        let before = session_entries(&store, &id);

        let mut outgoing = json!({});
        match materialise_fallback(&store, &id, &mut outgoing) {
            Ok(()) => {
                let submission = &outgoing["fallbackSubmission"];
                assert_eq!(submission["prompt"], "PUBLIC-fallback-prompt");
                for bytes in decoded_attachments(submission) {
                    assert!(
                        !String::from_utf8_lossy(&bytes).contains("### File:"),
                        "{submission}"
                    );
                }
            }
            Err(_) => assert!(outgoing["fallbackSubmission"].is_null()),
        }
        assert_eq!(session_entries(&store, &id), before);
        source.verify(&store, &id).unwrap();
    }
}
