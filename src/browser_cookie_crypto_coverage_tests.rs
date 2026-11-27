use super::*;

#[test]
fn invalid_and_host_mismatch_errors_have_distinct_display_messages() {
    for (error, expected) in [
        (DecodeError::Invalid, "Chrome cookie decryption failed"),
        (
            DecodeError::HostMismatch,
            "Chrome cookie host binding does not match",
        ),
    ] {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn valid_legacy_records_decode_at_the_size_limit_and_are_rejected_above_it() {
    const RECORD_OVERHEAD: usize = 3 + 12 + 16;

    let limit = crate::browser_auth::MAX_COOKIE_INPUT;
    let key_bytes = *b"PUBLIC-cookie-test-key-32-bytes!";
    let cipher = Aes256Gcm::new((&key_bytes).into());
    let key = CookieKey::Gcm(Zeroizing::new(key_bytes));

    for (record_len, nonce) in [(limit, *b"PUBLIC-iv-00"), (limit + 1, *b"PUBLIC-iv-01")] {
        // Authenticate both fixtures so removing the size check would let both decode.
        let mut plaintext = b"PUBLIC-".to_vec();
        plaintext.resize(record_len - RECORD_OVERHEAD, b'x');
        let ciphertext = cipher.encrypt(&nonce.into(), plaintext.as_slice()).unwrap();

        let mut record = b"v10".to_vec();
        record.extend_from_slice(&nonce);
        record.extend_from_slice(&ciphertext);
        assert_eq!(record.len(), record_len);

        for version in [b"v10", b"v11"] {
            record[..version.len()].copy_from_slice(version);
            let result = key.decrypt(&record, "PUBLIC-host.invalid", 23);
            if record_len == limit {
                assert_eq!(result.unwrap().as_bytes(), plaintext.as_slice());
            } else {
                assert_eq!(result.unwrap_err(), DecodeError::Invalid);
            }
        }
    }
}
