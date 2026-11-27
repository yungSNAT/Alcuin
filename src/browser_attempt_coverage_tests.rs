use super::*;

/// A consultation parent with one freshly reserved ChatGPT child, built like the module's own fixtures.
fn reserved_child() -> (tempfile::TempDir, SessionStore, String, Link) {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().join(".oracle"));
    let request =
        json!({"prompt":"PUBLIC recovery consultation","engine":"browser","model":"gpt-6-pro"});
    let environment = crate::config::Environment::from([
        (
            "ORACLE_HOME_DIR".into(),
            store.home().to_string_lossy().into(),
        ),
        (
            (if cfg!(windows) { "USERPROFILE" } else { "HOME" }).into(),
            root.path().to_string_lossy().into(),
        ),
    ]);
    let resolved = crate::execution::resolve(
        &request,
        &json!({"browser":{"cookieSync":false,"manualLogin":true}}),
        &environment,
        root.path(),
    )
    .unwrap();
    let meta = browser_runner::prepare(&store, &resolved, root.path()).unwrap();
    let id = meta["id"].as_str().unwrap().to_owned();
    let identity = ProcessIdentity::current().unwrap();
    browser_runner::start_owner(&store, &id, &identity).unwrap();
    let cancel = CancellationToken::new();
    let mut coordinator = Parent::begin(&store, &id, &identity, &cancel).unwrap();
    let link = coordinator.reserve_first(&cancel).unwrap();
    (root, store, id, link)
}

#[test]
fn an_unparseable_active_link_always_needs_recovery() {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().join(".oracle"));
    // No active attempt: nothing to recover.
    assert!(!needs_recovery(
        &store,
        &json!({"id":"PUBLIC-parent","browser":{"consultation":{"active":null}}})
    ));
    for active in [
        json!("PUBLIC-not-a-link"),
        json!({"version":1}),
        json!({"PUBLIC-unknown":true}),
        json!([1, 2, 3]),
    ] {
        let parent = json!({"id":"PUBLIC-parent","browser":{"consultation":{"active":active}}});
        assert!(needs_recovery(&store, &parent), "{active}");
    }
}

#[test]
fn a_fresh_profile_owner_needs_recovery_until_its_disposal_is_final() {
    let (_root, store, id, link) = reserved_child();
    let parent = store.require(&id).unwrap();
    assert!(
        !needs_recovery(&store, &parent),
        "a reserved child without browser evidence needs no recovery"
    );
    for (owner, disposal, expected) in [
        (Value::Null, Value::Null, false),
        (json!({"pid":1}), Value::Null, true),
        (json!({"pid":1}), json!({"disposition":"pending"}), true),
        (json!({"pid":1}), json!({"disposition":"removed"}), false),
        (json!({"pid":1}), json!({"disposition":"kept"}), false),
        (json!({"pid":1}), json!({"disposition":7}), true),
        (json!("PUBLIC-not-an-object"), Value::Null, false),
    ] {
        store
            .update(&link.child_id, |meta| {
                let mut runtime = meta["browser"]["runtime"].clone();
                if !runtime.is_object() {
                    runtime = json!({});
                }
                runtime["freshProfileOwner"] = owner.clone();
                runtime["freshProfileDisposal"] = disposal.clone();
                Ok(Some(json!({
                    "browser": merge_object(&meta["browser"], &json!({"runtime": runtime}))
                })))
            })
            .unwrap();
        let child = store.require(&link.child_id).unwrap();
        assert_eq!(child["browser"]["runtime"]["freshProfileOwner"], owner);
        assert_eq!(child["browser"]["attempt"], json!(link));
        let parent = store.require(&id).unwrap();
        assert_eq!(
            needs_recovery(&store, &parent),
            expected,
            "owner {owner} disposal {disposal}"
        );
    }
}
