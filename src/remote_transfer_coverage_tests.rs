use super::*;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

type Headers = Vec<(&'static str, String)>;

/// Serve the same HTTP response on every connection; record each request head.
async fn serve(
    status: &'static str,
    extra_headers: Headers,
    body: Vec<u8>,
) -> (reqwest::Url, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut head = Vec::new();
            let mut chunk = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => head.extend_from_slice(&chunk[..read]),
                }
            }
            recorded
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&head).into_owned());
            let mut reply = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            for (name, value) in &extra_headers {
                let _ = write!(reply, "{name}: {value}\r\n");
            }
            reply.push_str("\r\n");
            let mut bytes = reply.into_bytes();
            bytes.extend_from_slice(&body);
            let _ = stream.write_all(&bytes).await;
            let _ = stream.shutdown().await;
        }
    });
    let url = reqwest::Url::parse(&format!("http://127.0.0.1:{port}/bridge/")).unwrap();
    (url, requests)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn file_descriptor(body: &[u8], byte_size: usize) -> Value {
    json!({
        "kind": "file",
        "runId": "PUBLIC-run",
        "artifactId": "PUBLIC-art",
        "filename": "PUBLIC-report.txt",
        "byteSize": byte_size,
        "sha256": sha256_hex(body),
        "mimeType": "text/plain"
    })
}

struct RejectCase {
    name: &'static str,
    status: &'static str,
    headers: Headers,
    declared: usize,
    expected: &'static str,
}

#[tokio::test]
async fn transfer_rejects_bad_status_hash_header_and_content_length() {
    let body = b"PUBLIC-data".to_vec();
    let cases = vec![
        RejectCase {
            name: "status",
            status: "404 Not Found",
            headers: vec![],
            declared: body.len(),
            expected: "Remote artifact responded with status 404",
        },
        RejectCase {
            name: "hash header",
            status: "200 OK",
            headers: vec![("x-oracle-artifact-sha256", "0".repeat(64))],
            declared: body.len(),
            expected: "artifact sha256 header mismatch",
        },
        RejectCase {
            name: "content length",
            status: "200 OK",
            headers: vec![],
            declared: body.len() + 1,
            expected: "artifact content-length mismatch",
        },
    ];
    for case in cases {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("PUBLIC-report.txt");
        let (endpoint, _requests) = serve(case.status, case.headers, body.clone()).await;
        let descriptor_value = file_descriptor(&body, case.declared);
        let error = match transfer(
            &client(),
            &endpoint,
            None,
            &descriptor_value,
            &destination,
            false,
            &CancellationToken::new(),
        )
        .await
        {
            Ok(_) => panic!("{}: transfer unexpectedly succeeded", case.name),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains(case.expected),
            "{}: unexpected error {error:#}",
            case.name
        );
        assert!(
            !destination.exists(),
            "{}: destination was written",
            case.name
        );
    }
}

#[tokio::test]
async fn transfer_publishes_file_when_hash_header_and_length_match() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("PUBLIC-report.txt");
    let body = b"PUBLIC-file-contents".to_vec();
    let (endpoint, requests) = serve(
        "200 OK",
        vec![("x-oracle-artifact-sha256", sha256_hex(&body))],
        body.clone(),
    )
    .await;
    let descriptor_value = file_descriptor(&body, body.len());
    let transferred = transfer(
        &client(),
        &endpoint,
        Some("PUBLIC-token"),
        &descriptor_value,
        &destination,
        false,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let artifact = &transferred.artifact;
    assert!(transferred.warning.is_none());
    assert_eq!(artifact["kind"], "file");
    assert_eq!(artifact["sizeBytes"], json!(body.len()));
    assert_eq!(artifact["transfer"]["status"], "completed");
    assert_eq!(artifact["validation"]["type"], "generic");
    assert_eq!(artifact["validation"]["ok"], true);
    assert_eq!(artifact["sourceUrl"], "bridge-artifact");
    assert_eq!(artifact["filename"], artifact["label"]);
    let path = PathBuf::from(artifact["path"].as_str().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), body);
    let requests = requests.lock().unwrap();
    assert!(
        requests[0].starts_with("GET /bridge/runs/PUBLIC-run/artifacts/PUBLIC-art "),
        "{}",
        requests[0]
    );
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer public-token"),
        "{}",
        requests[0]
    );
}

#[tokio::test]
async fn transfer_copies_image_metadata_onto_image_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("PUBLIC-image.png");
    let body = b"PUBLIC-image-bytes".to_vec();
    let (endpoint, _requests) = serve("200 OK", vec![], body.clone()).await;
    let descriptor_value = json!({
        "kind": "image",
        "runId": "PUBLIC-run",
        "artifactId": "PUBLIC-img",
        "filename": "PUBLIC-image.png",
        "byteSize": body.len(),
        "sha256": sha256_hex(&body),
        "mimeType": "image/png",
        "image": {
            "width": 64,
            "height": 32,
            "prompt": "PUBLIC-prompt",
            "revisedPrompt": "PUBLIC-revised",
            "generationId": "PUBLIC-gen",
            "alt": "PUBLIC-alt"
        }
    });
    let transferred = transfer(
        &client(),
        &endpoint,
        None,
        &descriptor_value,
        &destination,
        false,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let artifact = &transferred.artifact;
    assert!(transferred.warning.is_none());
    assert_eq!(artifact["kind"], "image");
    let expected = crate::remote_protocol::image_metadata(&descriptor_value["image"]);
    let expected = expected.as_object().unwrap();
    for (field, value) in expected {
        assert_eq!(&artifact[field], value, "image field {field}");
    }
    if !expected.contains_key("filename") {
        assert!(artifact.get("filename").is_none());
    }
    let path = PathBuf::from(artifact["path"].as_str().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), body);
}
