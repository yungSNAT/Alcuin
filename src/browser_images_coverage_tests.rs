use super::*;

#[test]
fn generated_stems_collapse_separators_and_use_a_fallback() {
    for (source, expected) in [
        ("--PUBLIC / ☃ Image__!!!", "PUBLIC-Image__"),
        (" / ☃ !!! ", "generated"),
        ("", "generated"),
    ] {
        assert_eq!(generated_stem(source), expected);
    }
}

#[test]
fn inventory_propagates_malformed_download_url_errors() {
    let malformed = "https://[";
    let expected = browser_download::download_url(malformed).unwrap_err();
    let error = normalise_inventory(&[
        json!({"url":"https://chatgpt.com/backend-api/estuary/content?id=file_public"}),
        json!({"url":malformed}),
    ])
    .unwrap_err();

    assert_eq!(format!("{error:#}"), format!("{expected:#}"));
}

#[test]
fn requested_delivery_counts_only_distinct_nonblank_saved_paths() {
    let root = tempfile::tempdir().unwrap();
    let images = [
        json!({"url":"https://chatgpt.com/backend-api/estuary/content?id=file_public"}),
        json!({"url":"https://chatgpt.com/backend-api/estuary/content?id=file_public_2"}),
    ];
    let mut delivery = ImageDelivery::new(
        &json!({"generateImage":"PUBLIC-image.png"}),
        root.path(),
        root.path(),
        &images,
    )
    .unwrap();

    let first = json!({"path":root.path().join("PUBLIC-image.png")});
    delivery.saved = vec![
        first.clone(),
        first,
        json!({"path":" \t\n "}),
        json!({"path":17}),
        json!({}),
    ];
    assert_eq!(
        delivery.require_complete(&[]).unwrap_err().to_string(),
        "Required browser image delivery is incomplete; preserve the conversation and captured images for recovery."
    );

    delivery
        .saved
        .push(json!({"path":root.path().join("PUBLIC-image.2.png")}));
    delivery.require_complete(&[]).unwrap();
}

#[test]
fn requested_image_publication_and_recovery_check_output_indices_and_digests() {
    let root = tempfile::tempdir().unwrap();
    let options = json!({"generateImage":"PUBLIC-image.png"});
    let images = [json!({"url":"https://chatgpt.com/backend-api/estuary/content?id=file_public"})];
    let origin = json!({
        "mode":"local",
        "targetId":"PUBLIC-target",
        "assistantMessageId":"PUBLIC-message",
        "assistantTurnId":"PUBLIC-turn",
    });
    let delivery_at = |offset| {
        ImageDelivery::after_previous(&options, root.path(), root.path(), &images, offset).unwrap()
    };
    let mut publisher = delivery_at(0);
    let mut candidate = publisher.generated[0].clone();
    candidate["origin"] = origin.clone();
    let bytes: &[u8] = b"\x89PNG\r\n\x1a\nPUBLIC-image";
    let cancel = CancellationToken::new();

    let mut overflowing = delivery_at(usize::MAX);
    let error = overflowing.publish(bytes, &candidate, &cancel).unwrap_err();
    assert_eq!(error.to_string(), "Image index overflow");
    assert!(overflowing.saved_images().is_empty());
    assert!(root.path().read_dir().unwrap().next().is_none());

    let receipt = publisher.publish(bytes, &candidate, &cancel).unwrap();
    let destination = root.path().join("PUBLIC-image.png");
    assert_eq!(receipt["path"], json!(destination));

    let error = overflowing
        .restore_saved(std::slice::from_ref(&receipt), &origin, false)
        .unwrap_err();
    assert_eq!(error.to_string(), "Image index overflow");
    assert!(overflowing.saved_images().is_empty());

    // Keep both digest fields in agreement so the file verifier, not the
    // manifest consistency check, rejects the incorrect digest.
    let mut bad_receipt = receipt.clone();
    let mut publication: artifacts::PublishedArtifact =
        serde_json::from_value(receipt["publicationReceipt"].clone()).unwrap();
    publication.sha256 = "0".repeat(64);
    assert_ne!(receipt["sha256"], json!(publication.sha256));
    bad_receipt["sha256"] = json!(publication.sha256);
    bad_receipt["publicationReceipt"] = json!(publication);

    let expected = artifacts::verify_streamed_file(
        &mut std::fs::File::open(&destination).unwrap(),
        publication.size_bytes,
        &publication.sha256,
        None,
    )
    .unwrap_err();
    let mut recovered = delivery_at(0);
    let error = recovered
        .restore_saved(&[bad_receipt], &origin, false)
        .unwrap_err();
    assert_eq!(format!("{error:#}"), format!("{expected:#}"));
    assert!(recovered.saved_images().is_empty());

    recovered
        .restore_saved(std::slice::from_ref(&receipt), &origin, false)
        .unwrap();
    assert_eq!(recovered.saved_images(), std::slice::from_ref(&receipt));
    recovered.require_complete(&[]).unwrap();
    assert_eq!(std::fs::read(destination).unwrap(), bytes);
}
