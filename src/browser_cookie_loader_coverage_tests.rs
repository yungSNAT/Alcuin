#![cfg(target_os = "linux")]

use super::*;
use serde_json::json;

/// Linux synthetic ciphertext for `PUBLIC-cookie-value` bound to `.chatgpt.com` (meta version 24).
const PUBLIC_LINUX_CIPHERTEXT_HEX: &str = "763130b6bfa63c66774b16309c3b492b56bc030f5dbcf34e106dd42f933295edd1dec4e164c358c7f2b717359ba1a626ceb27fc4797ce337e7360a34da93a063f1b9ca";

fn decode_hex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn keys_reject_unversioned_blobs_and_v11_blobs_without_a_linux_keyring_key() {
    let v10 = decode_hex(PUBLIC_LINUX_CIPHERTEXT_HEX);
    assert!(v10.starts_with(b"v10"));

    let primary_only = Keys {
        primary: CookieKey::linux(b"peanuts"),
        current: None,
        empty: None,
    };
    // Sanity: the v10 blob decrypts with the primary Linux key.
    let decrypted = primary_only.decrypt(&v10, ".chatgpt.com", 24);
    assert_eq!(
        decrypted.as_deref().map(String::as_str),
        Some("PUBLIC-cookie-value")
    );

    // The same payload tagged v11 must not fall back to the primary key when
    // neither a keyring-derived key nor the empty-password key is available.
    let mut v11 = v10.clone();
    v11[..3].copy_from_slice(b"v11");
    assert!(primary_only.decrypt(&v11, ".chatgpt.com", 24).is_none());

    // Blobs without a v10/v11 prefix are never decrypted, whatever keys exist.
    let all_keys = Keys {
        primary: CookieKey::linux(b"peanuts"),
        current: Some(CookieKey::linux(b"peanuts")),
        empty: Some(CookieKey::linux(b"")),
    };
    let mut unversioned = v10.clone();
    unversioned[..3].copy_from_slice(b"v12");
    for blob in [
        unversioned.as_slice(),
        b"PUBLIC-plain-value".as_slice(),
        b"".as_slice(),
        b"v1".as_slice(),
    ] {
        assert!(all_keys.decrypt(blob, ".chatgpt.com", 24).is_none());
    }
}

#[tokio::test]
async fn unsupported_or_unknown_linux_keyring_backends_fail_before_reading_cookies() {
    for (backend, expected) in [
        (
            "kwallet",
            "KWallet Chrome cookie extraction remains unavailable",
        ),
        (
            "  kwallet  ",
            "KWallet Chrome cookie extraction remains unavailable",
        ),
        (
            "PUBLIC-unknown-backend",
            "Unknown Chrome cookie keyring backend",
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let (source, mut env) = synthetic_source(home.path());
        env.insert("SWEET_COOKIE_LINUX_KEYRING".into(), backend.into());
        let original = std::fs::read(&source).unwrap();
        let temporary = home.path().join("PUBLIC-snapshots");

        let error = read_native(
            &json!({"cookieSync":true,"chromeCookiePath":source}),
            &env,
            home.path(),
            Instant::now() + std::time::Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(expected),
            "backend {backend:?}: unexpected error {message:?}"
        );
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert!(
            !temporary.exists(),
            "backend {backend:?}: no snapshot should be created"
        );
    }
}
