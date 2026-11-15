use alcuin::artifacts::sha256;
use serde_json::Value;
use std::{fs, path::Path};

/// The Temporary Chat predicate is the reviewed one its provenance record names, so an edit to
/// either the script or the record without the other fails here.
#[test]
fn temporary_chat_predicate_matches_its_provenance_record() {
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("browser-scripts");
    let record: Value = serde_json::from_str(
        &fs::read_to_string(scripts.join("temporary-chat-source.json")).unwrap(),
    )
    .unwrap();
    // The committed text has LF line ends; a Windows checkout may turn them into CRLF.
    let script = fs::read_to_string(scripts.join("temporary-chat.js"))
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(
        sha256(script.as_bytes()),
        record["predicateSha256"].as_str().unwrap()
    );
}
