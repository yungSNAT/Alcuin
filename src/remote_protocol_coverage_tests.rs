use super::*;
use crate::remote_result::{Phase, read_phase, save, session_id};
use std::fs;
use tokio_util::sync::CancellationToken;

const RUN_ID: &str = "12345678-1234-1234-1234-123456789abc";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\
    \0\0\0\x0dIHDR\0\0\0\x01\0\0\0\x01\x08\x04\0\0\0\xb5\x1c\x0c\x02\
    \0\0\0\x0bIDAT\x78\xda\x63\x64\xf8\x0f\0\x01\x05\x01\x01\x27\x18\xe3\x66\
    \0\0\0\0IEND\xae\x42\x60\x82";

fn write_receipt(path: &Path, kind: &str, bytes: &[u8]) -> Value {
    fs::write(path, bytes).unwrap();
    json!({
        "kind": kind,
        "path": path,
        "sizeBytes": bytes.len(),
        "sha256": artifacts::sha256(bytes),
        "nativeIdentity": NativeEntry::open(path, false).unwrap().identity
    })
}

#[test]
fn image_metadata_keeps_only_safe_dimensions_and_file_ids() {
    let longest_id = format!("file-PUBLIC-{}", "a".repeat(193));
    let oversized_id = format!("{longest_id}a");
    for (input, expected) in [
        (
            json!({"width": 1, "height": 9_007_199_254_740_991_u64, "fileId": longest_id}),
            json!({"width": 1.0, "height": 9_007_199_254_740_991.0, "fileId": longest_id}),
        ),
        (
            json!({"width": 0, "height": 12, "fileId": "FiLe_PUBLIC-2"}),
            json!({"height": 12.0, "fileId": "FiLe_PUBLIC-2"}),
        ),
        (
            json!({"width": 1.5, "height": -1, "fileId": "file-"}),
            json!({}),
        ),
        (
            json!({"width": 9_007_199_254_740_992_u64, "height": "12", "fileId": oversized_id}),
            json!({}),
        ),
        (json!({"fileId": "file-PUBLIC/a"}), json!({})),
        (json!({"fileId": "file-PUBLIC-é"}), json!({})),
        (json!({"fileId": "PUBLIC-unprefixed"}), json!({})),
    ] {
        assert_eq!(image_metadata(&input), expected, "{input}");
    }
}

#[test]
fn required_image_delivery_allows_file_warnings_but_not_image_warnings() {
    for (failed_kind, warning_code) in [
        ("file", "remote-artifact-registration-failed"),
        ("image", "remote-image-registration-failed"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let store = SessionStore::new(home.path().join(".oracle"));
        let id = session_id(RUN_ID).unwrap();
        store
            .create(json!({"prompt": "PUBLIC-recovery"}), home.path(), Some(&id))
            .unwrap();
        let directory = store.directory(&id).unwrap().join("artifacts");
        fs::create_dir(&directory).unwrap();

        let image_path = directory.join("PUBLIC-image.png");
        let mut image = write_receipt(&image_path, "image", PNG);
        image["mimeType"] = json!("image/png");
        image["width"] = json!(1);
        image["height"] = json!(1);
        image["fileId"] = json!("FiLe_PUBLIC-image");
        image["sourceUrl"] = json!("sandbox:/mnt/data/PUBLIC-image.png");
        image["url"] = json!("https://example.invalid/backend-api/PUBLIC-unused");

        let file_path = directory.join("PUBLIC-file.txt");
        let mut file = write_receipt(&file_path, "file", b"PUBLIC-file");
        file["url"] = json!("https://example.invalid/backend-api/PUBLIC-file");

        // Keep the rejected path present so recovery can canonicalize it and check its warning.
        let rejected_path = home.path().join("PUBLIC-outside.bin");
        let rejected = write_receipt(&rejected_path, failed_kind, b"PUBLIC-outside");
        let captured = json!({
            "answerText": format!("PUBLIC-response Saved to: {}", image_path.display()),
            "answerMarkdown": format!("PUBLIC-link: {}", file_path.display()),
            "warnings": [cleanup_warning("image", "local", &image_path)],
            "artifacts": [
                {"kind": "image"},
                {"kind": "file", "path": ""},
                {"kind": "PUBLIC-unsupported", "path": image_path},
                image,
                {"kind": "image", "path": image_path, "sha256": artifacts::sha256(b"PUBLIC-duplicate")},
                file,
                rejected
            ]
        });

        let failed_image = failed_kind == "image";
        if failed_image {
            let Err(error) = produced(&store, RUN_ID, captured.clone(), true) else {
                panic!("required delivery accepted a failed image alongside a registered image");
            };
            assert_eq!(
                error.to_string(),
                "Required generated image delivery is incomplete"
            );
        }

        let output = produced(&store, RUN_ID, captured.clone(), !failed_image).unwrap();
        assert_eq!(output.captured, captured);
        let rows = output.artifacts.as_array().unwrap();
        assert_eq!(rows.len(), 2);

        let descriptor = &rows[0]["descriptor"];
        assert_eq!(
            rows[0]["filePath"],
            json!(image_path.canonicalize().unwrap())
        );
        assert_eq!(descriptor["kind"], "image");
        assert_eq!(descriptor["filename"], "public-image.png");
        assert_eq!(descriptor["byteSize"], json!(PNG.len()));
        assert_eq!(descriptor["sha256"], artifacts::sha256(PNG));
        assert_eq!(descriptor["mimeType"], "image/png");
        assert_eq!(descriptor["sourceUrlKind"], "sandbox");
        assert_eq!(descriptor["transferStatus"], "ready");
        assert_eq!(
            descriptor["image"],
            json!({"width": 1.0, "height": 1.0, "fileId": "FiLe_PUBLIC-image"})
        );
        let file_descriptor = &rows[1]["descriptor"];
        assert_eq!(file_descriptor["kind"], "file");
        assert_eq!(file_descriptor["sourceUrlKind"], "chatgpt-file-endpoint");
        assert!(file_descriptor.get("image").is_none());

        assert_eq!(output.result["answerText"], "PUBLIC-response");
        assert_eq!(
            output.result["answerMarkdown"],
            "PUBLIC-link: PUBLIC-file.txt"
        );
        let warnings = output.result["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 2);
        assert_eq!(warnings[0], cleanup_warning("image", "host", &image_path));
        assert_eq!(warnings[1]["code"], warning_code);
        assert_eq!(warnings[1]["severity"], "warning");
        let message = warnings[1]["message"].as_str().unwrap();
        assert!(message.contains("could not prepare public-outside.bin for transfer"));
        assert!(!message.contains(rejected_path.to_str().unwrap()));

        let cancel = CancellationToken::new();
        save(
            &store,
            RUN_ID,
            &captured,
            &output.result,
            &output.artifacts,
            Phase::Capture,
            &cancel,
        )
        .unwrap();
        let record = read_phase(&store, RUN_ID, Phase::Capture).unwrap();
        if failed_image {
            verify_recovered(&store, RUN_ID, &record, &captured, false, &cancel, None).unwrap();
            let error = verify_recovered(&store, RUN_ID, &record, &captured, true, &cancel, None)
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "Required recovered image delivery is incomplete"
            );
        } else {
            verify_recovered(&store, RUN_ID, &record, &captured, true, &cancel, None).unwrap();
        }
        assert_eq!(
            record.native()["artifacts"][0]["descriptor"]["artifactId"],
            descriptor["artifactId"]
        );
    }
}
