//! Coverage tests for token files, endpoint normalisation and configured approval waits.

use super::*;

fn public_env() -> Environment {
    Environment::from_iter([("HOME".into(), "PUBLIC-home".into())])
}

#[test]
fn read_token_file_accepts_one_trailing_line_ending_and_rejects_bad_files() {
    let dir = tempfile::tempdir().unwrap();
    for (name, contents, expected) in [
        ("lf", "PUBLIC-token\n", "PUBLIC-token"),
        ("crlf", "PUBLIC-token\r\n", "PUBLIC-token"),
        ("bare", "PUBLIC-token", "PUBLIC-token"),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, contents).unwrap();
        assert_eq!(read_token_file(&path).unwrap(), expected, "{name}");
    }

    for (name, contents, message) in [
        (
            "multi",
            "PUBLIC-a\nPUBLIC-b",
            "must hold the token on one line",
        ),
        (
            "double",
            "PUBLIC-token\n\n",
            "must hold the token on one line",
        ),
        ("blank", "   \n", "is empty"),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, contents).unwrap();
        let error = read_token_file(&path).unwrap_err();
        assert!(error.to_string().contains(message), "{name}: {error:#}");
    }

    let missing = dir.path().join("missing");
    let error = read_token_file(&missing).unwrap_err();
    assert!(
        error.to_string().contains("Cannot read token file"),
        "{error:#}"
    );
}

#[test]
fn configured_ipv6_hosts_are_bracketed_before_parsing() {
    for (host, expected) in [
        ("::1", "::1"),
        ("2001:db8::1", "2001:db8::1"),
        ("[::1]", "::1"),
        ("localhost", "localhost"),
    ] {
        let parsed = endpoint(&json!({"host": host, "port": 9222})).unwrap();
        assert_eq!(parsed, json!({"host": expected, "port": 9222}), "{host}");
    }
    let config = json!({"browser": {"remoteChrome": {"host": "::1", "port": 1234}}});
    let browser =
        browser_config(&config, &json!({}), &public_env(), Path::new("PUBLIC-cwd")).unwrap();
    assert_eq!(
        browser["remoteChrome"],
        json!({"host": "::1", "port": 1234})
    );
    assert_eq!(browser["manualLogin"], false);
}

#[test]
fn configured_approval_wait_accepts_numbers_strings_and_blanks() {
    for (configured, expected) in [
        (json!(15000), 15000),
        (json!(""), 20000),
        (json!("   "), 20000),
        (json!("5s"), 5000),
        (Value::Null, 20000),
    ] {
        let config = json!({"browser": {"approvalWaitMs": configured.clone()}});
        let browser =
            browser_config(&config, &json!({}), &public_env(), Path::new("PUBLIC-cwd")).unwrap();
        assert_eq!(browser["approvalWaitMs"], expected, "{configured}");
    }
}

#[test]
fn configured_approval_wait_rejects_invalid_values() {
    for (configured, message) in [
        (json!(true), "Invalid configured browser approval wait"),
        (
            json!(["PUBLIC-list"]),
            "Invalid configured browser approval wait",
        ),
        (
            json!("PUBLIC-bogus"),
            "Invalid configured browser approval wait",
        ),
        (json!(1.5), "positive integer"),
        (json!(0), "positive integer"),
    ] {
        let config = json!({"browser": {"approvalWaitMs": configured.clone()}});
        let error = browser_config(&config, &json!({}), &public_env(), Path::new("PUBLIC-cwd"))
            .unwrap_err();
        assert!(
            format!("{error:#}").contains(message),
            "{configured}: {error:#}"
        );
    }
}
