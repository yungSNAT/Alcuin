#![cfg(target_os = "linux")]

use super::*;

#[tokio::test]
async fn discovery_skips_queued_descendants_of_a_replaced_root() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("PUBLIC-root");
    let retired = home.path().join("PUBLIC-retired");
    let branch = root.join("PUBLIC-branch");
    let subtree = branch.join("PUBLIC-subtree");
    let fallback = home.path().join("PUBLIC-fallback");

    fs::create_dir_all(&subtree).unwrap();
    fs::create_dir(&fallback).unwrap();

    // The root contributes one entry and the branch contributes 127, so
    // discovery yields on the last branch entry regardless of listing order.
    for index in 0..126 {
        fs::write(branch.join(format!("PUBLIC-entry-{index:03}")), []).unwrap();
    }
    // Scanning this subtree instead of skipping it would cause a second yield.
    for index in 0..128 {
        fs::write(subtree.join(format!("PUBLIC-entry-{index:03}")), []).unwrap();
    }

    let fallback_metadata = fallback.join("DevToolsActivePort");
    fs::write(
        &fallback_metadata,
        "9222\n/devtools/browser/PUBLIC-fallback\n",
    )
    .unwrap();

    let original_root = NativeEntry::open(&root, true).unwrap();
    let original_branch = NativeEntry::open(&branch, true).unwrap();
    let roots = [root.clone(), fallback.clone()];
    let cancel = CancellationToken::new();
    let discovery = discover(
        &roots,
        "127.0.0.1",
        9222,
        Instant::now() + Duration::from_secs(2),
        &cancel,
    );
    tokio::pin!(discovery);

    assert!(
        futures_util::poll!(discovery.as_mut()).is_pending(),
        "expected discovery to yield while listing the branch"
    );

    // Preserve the immediate parent's identity so opening its child still
    // works, but leave discovery's saved root handle pointing at the old root.
    fs::rename(&root, &retired).unwrap();
    fs::create_dir(&root).unwrap();
    fs::rename(retired.join("PUBLIC-branch"), &branch).unwrap();
    assert!(!original_root.matches(&root));
    assert!(original_branch.matches(&branch));

    let std::task::Poll::Ready(result) = futures_util::poll!(discovery.as_mut()) else {
        panic!("discovery scanned another batch below the replaced root");
    };
    let candidate = result
        .unwrap()
        .expect("expected metadata from the next root");
    assert_eq!(candidate.path, fallback_metadata);
    assert_eq!(candidate.profile_root, fallback);
    assert_eq!(
        candidate.endpoint.browser_ws_endpoint,
        "ws://127.0.0.1:9222/devtools/browser/PUBLIC-fallback"
    );
}
