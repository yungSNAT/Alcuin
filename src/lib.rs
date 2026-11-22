/// A script from `browser-scripts/`, as the browser receives it: `build.rs` splices the ownership
/// prelude into the scripts that mark its place.
macro_rules! browser_script {
    ($name:literal) => {
        include_str!(concat!(env!("OUT_DIR"), "/browser-scripts/", $name))
    };
}
pub mod artifacts;
pub mod bridge;
pub mod bridge_host;
pub mod browser;
mod browser_attach_running;
pub(crate) mod browser_attempt;
pub(crate) mod browser_auth;
pub mod browser_bundle;
pub mod browser_chrome;
pub mod browser_commit;
mod browser_cookie_crypto;
mod browser_cookie_extract;
mod browser_cookie_keys;
mod browser_cookie_loader;
mod browser_cookie_sources;
pub mod browser_detect;
pub mod browser_disposition;
pub mod browser_dom;
pub mod browser_download;
mod browser_grok;
pub mod browser_harvest;
pub mod browser_health;
pub mod browser_historical;
pub mod browser_images;
pub mod browser_inputs;
pub mod browser_inspection;
mod browser_log;
pub mod browser_output;
mod browser_page;
mod browser_privacy;
pub mod browser_profile;
mod browser_profile_copy;
mod browser_profile_tree;
pub mod browser_prompt;
pub(crate) mod browser_provider;
pub mod browser_recovery;
pub mod browser_renderer;
pub mod browser_research;
mod browser_retirement;
pub mod browser_runner;
mod browser_search;
pub mod browser_sessions;
mod browser_signal;
pub mod browser_tabs;
pub mod browser_target;
mod browser_turns;
mod browser_window;
pub mod cdp;
pub mod cli;
mod cli_execution;
mod cli_format;
pub mod config;
pub mod docs_check;
pub mod execution;
pub mod followup;
mod grounding;
pub mod mcp;
pub mod native_fs;
mod native_reply;
pub mod notify;
pub mod perf;
pub mod policy;
pub mod process;
pub mod project_sources;
pub mod prompts;
pub mod providers;
pub mod remote_client;
pub mod remote_driver;
pub mod remote_host;
pub mod remote_json;
mod remote_protocol;
pub mod remote_result;
pub mod remote_service;
pub mod remote_session;
pub mod remote_slots;
mod remote_transfer;
mod response_stream;
mod restart;
pub mod secret_scan;
mod session_attach;
mod session_display;
pub mod sessions;
pub mod storage;
pub mod tokenizer;
mod window_keeper;

#[cfg(test)]
mod browser_script_tests {
    /// Pins the text the browser receives for the scripts that read target ownership. They run
    /// in a signed-in ChatGPT session, so a change to that text is a reviewed change: update the
    /// hash with it.
    #[test]
    fn injected_ownership_scripts_are_pinned() {
        let text = [
            browser_script!("attachment-evidence.js"),
            browser_script!("attachment-ready.js"),
            browser_script!("completion-install.js"),
            browser_script!("copy-markdown.js"),
            browser_script!("file-input-prepare.js"),
            browser_script!("inspection-finish.js"),
            browser_script!("inspection-prepare.js"),
            browser_script!("model.js"),
            browser_script!("owned_commit.js"),
            browser_script!("prompt-check.js"),
            browser_script!("prompt-prepare.js"),
            browser_script!("send-focus.js"),
            browser_script!("send-prepare.js"),
            browser_script!("submission.js"),
            browser_script!("thinking.js"),
        ]
        .map(|script| {
            // A spliced prelude takes its script's line ends.
            let crlf = script.matches("\r\n").count();
            assert!(
                crlf == 0 || crlf == script.matches('\n').count(),
                "mixed line ends"
            );
            let lf = script.replace("\r\n", "\n");
            format!("{}\n{lf}", lf.len())
        })
        .concat();
        assert_eq!(
            crate::artifacts::sha256(text.as_bytes()),
            "cbebb6be7d02558cbb172047651a312aa85a79a044fa700d5b8bbee91253ae02"
        );
    }
}
