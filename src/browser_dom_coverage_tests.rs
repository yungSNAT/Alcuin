use super::*;

#[test]
fn guessed_mime_types_follow_the_reference_extension_table() {
    for (name, expected) in [
        ("PUBLIC-notes.txt", "text/plain"),
        ("PUBLIC-README.MD", "text/markdown"),
        ("PUBLIC-table.csv", "text/csv"),
        ("PUBLIC-data.json", "application/json"),
        ("PUBLIC-app.js", "text/javascript"),
        ("PUBLIC-view.jsx", "text/javascript"),
        ("PUBLIC-lib.ts", "text/typescript"),
        ("PUBLIC-view.tsx", "text/typescript"),
        ("PUBLIC-tool.py", "text/x-python"),
        ("PUBLIC-Main.java", "text/x-java"),
        ("PUBLIC-main.c", "text/x-c"),
        ("PUBLIC-main.h", "text/x-c"),
        ("PUBLIC-main.cpp", "text/x-c++"),
        ("PUBLIC-main.hpp", "text/x-c++"),
        ("PUBLIC-run.sh", "text/x-sh"),
        ("PUBLIC-run.bash", "text/x-sh"),
        ("PUBLIC-page.html", "text/html"),
        ("PUBLIC-style.css", "text/css"),
        ("PUBLIC-feed.xml", "text/xml"),
        ("PUBLIC-config.yaml", "text/yaml"),
        ("PUBLIC-config.yml", "text/yaml"),
        ("PUBLIC-paper.pdf", "application/pdf"),
        ("PUBLIC-letter.doc", "application/msword"),
        (
            "PUBLIC-letter.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        ("PUBLIC-sheet.xls", "application/vnd.ms-excel"),
        (
            "PUBLIC-sheet.xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
        ("PUBLIC-deck.ppt", "application/vnd.ms-powerpoint"),
        (
            "PUBLIC-deck.pptx",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ),
        ("PUBLIC-shot.png", "image/png"),
        ("PUBLIC-shot.jpg", "image/jpeg"),
        ("PUBLIC-shot.JPEG", "image/jpeg"),
        ("PUBLIC-anim.gif", "image/gif"),
        ("PUBLIC-icon.svg", "image/svg+xml"),
        ("PUBLIC-photo.webp", "image/webp"),
        ("PUBLIC-bundle.zip", "application/zip"),
        ("PUBLIC-bundle.tar", "application/x-tar"),
        ("PUBLIC-bundle.tar.gz", "application/gzip"),
        ("PUBLIC-bundle.7z", "application/x-7z-compressed"),
        ("PUBLIC-binary.unknown", "application/octet-stream"),
        ("PUBLIC-no-extension", "application/octet-stream"),
        (".hidden", "application/octet-stream"),
    ] {
        assert_eq!(guess_mime_type(name), expected, "{name}");
    }
}

#[test]
fn attachment_expectations_escape_regex_metacharacters_in_names() {
    let chars = r"\p{L}\p{N}\p{M}._-";
    let boundary = format!("(?=$|[^{chars}])");
    let start = format!("(?:^|[^{chars}])");
    for (name, generated, expected_name, expected_stem, expected_pattern, image) in [
        (
            "PUBLIC (Draft)   v1.2+x.txt",
            false,
            "public (draft) v1.2+x.txt",
            "public (draft) v1.2+x",
            format!(
                r"{start}public \(draft\) v1\.2\+x(?:\s*\([0-9]+(?:-[0-9]+)*\))?\.txt{boundary}"
            ),
            false,
        ),
        (
            "PUBLIC[a]^b$|c",
            true,
            "public[a]^b$|c",
            "public[a]^b$|c",
            format!(r"{start}public\[a\]\^b\$\|c{boundary}"),
            false,
        ),
        (
            "PUBLIC   Shot{1}*?.PNG",
            false,
            "public shot{1}*?.png",
            "public shot{1}*?",
            format!(r"{start}public shot\{{1\}}\*\?(?:\s*\([0-9]+(?:-[0-9]+)*\))?\.png{boundary}"),
            true,
        ),
    ] {
        let expectation = attachment_expectation_for(name, generated);
        assert_eq!(expectation["name"], expected_name, "{name}");
        assert_eq!(expectation["stem"], expected_stem, "{name}");
        assert_eq!(
            expectation["namePattern"],
            json!(expected_pattern),
            "{name}"
        );
        assert_eq!(expectation["generatedBundle"], generated, "{name}");
        assert_eq!(expectation["image"], image, "{name}");
    }
}

#[test]
fn snapshot_writes_a_verified_durable_copy_and_refuses_changed_sources() {
    let root = tempfile::tempdir().unwrap();
    let content = b"PUBLIC-fixture snapshot contents\n";
    let source = root.path().join("PUBLIC-source.txt");
    fs::write(&source, content).unwrap();
    let mut file = UploadFile::open(&source).unwrap();

    let destination = root.path().join("PUBLIC-snapshot.txt");
    let mut captured = file.snapshot(&destination).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), content);
    let original = file.manifest();
    let copy = captured.manifest();
    assert_eq!(copy["bytes"], content.len());
    assert_eq!(copy["bytes"], original["bytes"]);
    assert_eq!(copy["sha256"], original["sha256"]);
    assert_eq!(copy["sha256"], json!(crate::artifacts::sha256(content)));
    assert_eq!(copy["name"], "PUBLIC-snapshot.txt");
    assert_eq!(copy["path"], json!(fs::canonicalize(&destination).unwrap()));
    assert_eq!(captured.contents().unwrap(), content);
    assert!(!captured.generated_bundle);
    assert!(!captured.data_transfer);

    fs::write(
        &source,
        b"PUBLIC-fixture changed after capture, now longer\n",
    )
    .unwrap();
    let refused = root.path().join("PUBLIC-refused.txt");
    assert!(file.snapshot(&refused).is_err());
    assert!(!refused.exists());
}
