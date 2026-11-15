use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub type Environment = BTreeMap<String, String>;
pub fn oracle_home(env: &Environment) -> Result<PathBuf> {
    if let Some(path) = env.get("ORACLE_HOME_DIR") {
        return Ok(PathBuf::from(path));
    }
    let home = if cfg!(windows) {
        env.get("USERPROFILE")
    } else {
        env.get("HOME")
    };
    Ok(
        PathBuf::from(home.context("Unable to resolve home directory; set ORACLE_HOME_DIR")?)
            .join(".oracle"),
    )
}
/// Set by synthetic proofs so a missing profile option can never reach the signed-in profile.
pub const REFUSE_DEFAULT_PROFILE_ENV: &str = "ORACLE_REFUSE_DEFAULT_BROWSER_PROFILE";
/// The first non-blank candidate, else `<home>/.oracle/browser-profile`, which
/// `ORACLE_REFUSE_DEFAULT_BROWSER_PROFILE=1` refuses.
pub fn manual_login_profile_dir<'a>(
    candidates: impl IntoIterator<Item = Option<&'a str>>,
    env: &Environment,
    home: &str,
) -> Result<PathBuf> {
    if let Some(dir) = candidates
        .into_iter()
        .flatten()
        .map(crate::policy::trim)
        .find(|dir| !dir.is_empty())
    {
        return Ok(PathBuf::from(dir));
    }
    anyhow::ensure!(
        env.get(REFUSE_DEFAULT_PROFILE_ENV).map(String::as_str) != Some("1"),
        "{REFUSE_DEFAULT_PROFILE_ENV}=1 refuses the default signed-in browser profile; pass an explicit profile directory."
    );
    Ok(PathBuf::from(home).join(".oracle/browser-profile"))
}
pub fn runtime_environment() -> Environment {
    // Read application variables by name; never enumerate unrelated credentials.
    [
        "ORACLE_HOME_DIR",
        REFUSE_DEFAULT_PROFILE_ENV,
        "HOME",
        "USERPROFILE",
        "PATH",
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
        "APPDATA",
        "LANG",
        "LC_ALL",
        "TEMP",
        "TMP",
        "TMPDIR",
        "ORACLE_ENGINE",
        "ORACLE_BROWSER_PROFILE_DIR",
        "ORACLE_BROWSER_PORT",
        "ORACLE_BROWSER_DEBUG_PORT",
        "ORACLE_BROWSER_ALLOW_COOKIE_ERRORS",
        "ORACLE_BROWSER_MAX_CONCURRENT_TABS",
        "ORACLE_MCP_ALLOW_EXTERNAL_INPUT",
        "ORACLE_MCP_ALL_SESSIONS",
        "ORACLE_MAX_FILE_SIZE_BYTES",
        "ORACLE_MIN_PROMPT_CHARS",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENROUTER_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "AZURE_OPENAI_ENDPOINT",
        "AZURE_OPENAI_DEPLOYMENT",
        "AZURE_OPENAI_API_VERSION",
        "ORACLE_REMOTE_HOST",
        "ORACLE_REMOTE_TOKEN",
        "ORACLE_BROWSER_APPROVAL_WAIT",
        "ORACLE_BROWSER_ATTACHMENT_TIMEOUT",
        "ORACLE_FORCE_TUI",
        "ORACLE_BROWSER_COOKIE_NAMES",
        "ORACLE_COOKIE_LOAD_TIMEOUT_MS",
        "SWEET_COOKIE_LINUX_KEYRING",
        "CHROME_PATH",
        "LIGHTHOUSE_CHROMIUM_PATH",
        "ORACLE_CHROME_NO_SANDBOX",
        "DISPLAY",
        "XAUTHORITY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "XDG_CONFIG_HOME",
        "DBUS_SESSION_BUS_ADDRESS",
        "SSH_AUTH_SOCK",
        "ORACLE_DETACHED_START_GATE",
        "ORACLE_BRIDGE_START_GATE",
        "ORACLE_NO_DETACH",
        "ORACLE_NOTIFY",
        "ORACLE_PERF_TRACE",
        "ORACLE_BAR_CONFIRM_CYCLES",
        "ORACLE_TERMINAL_MIN_STABLE_MS",
        "ORACLE_NOTIFY_SOUND",
        "ORACLE_DISABLE_NOTIFICATIONS",
        "CI",
        "SSH_CONNECTION",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
    .collect()
}
pub fn deep_merge(base: &Value, overlay: &Value) -> Value {
    let (Some(base), Some(overlay)) = (base.as_object(), overlay.as_object()) else {
        return overlay.clone();
    };
    let mut output = base.clone();
    for (key, value) in overlay {
        output.insert(
            key.clone(),
            deep_merge(output.get(key).unwrap_or(&Value::Null), value),
        );
    }
    Value::Object(output)
}
pub fn sanitise_project(config: &Value) -> Value {
    let mut output = json!({});
    for key in [
        "engine",
        "model",
        "search",
        "maxFileSizeBytes",
        "notify",
        "heartbeatSeconds",
        "filesReport",
        "background",
        "promptSuffix",
    ] {
        if let Some(value) = config.get(key) {
            output[key] = value.clone();
        }
    }
    if let Some(browser) = config.get("browser") {
        output["browser"] = json!({});
        for key in [
            "attachRunning",
            "timeoutMs",
            "inputTimeoutMs",
            "approvalWaitMs",
            "attachmentTimeoutMs",
            "assistantRecheckDelayMs",
            "assistantRecheckTimeoutMs",
            "reuseChromeWaitMs",
            "profileLockTimeoutMs",
            "maxConcurrentTabs",
            "autoReattachDelayMs",
            "autoReattachIntervalMs",
            "autoReattachTimeoutMs",
            "cookieSyncWaitMs",
            "hideWindow",
            "keepBrowser",
            "modelStrategy",
            "thinkingTime",
            "researchMode",
            "archiveConversations",
            "manualLogin",
        ] {
            if let Some(value) = browser.get(key) {
                output["browser"][key] = value.clone();
            }
        }
        let url = browser
            .get("chatgptUrl")
            .filter(|v| !v.is_null())
            .or_else(|| browser.get("url"));
        if let Some(url) = url
            && (url.is_null() || url.as_str().is_some_and(trusted_chatgpt_url))
        {
            output["browser"]["chatgptUrl"] = url.clone();
            output["browser"]["url"] = url.clone();
        }
    }
    output
}
pub fn trusted_chatgpt_url(raw: &str) -> bool {
    crate::browser::provider_url(raw).is_some()
}
#[derive(Clone, Debug)]
pub struct LoadedConfig {
    pub config: Value,
    pub paths: Vec<PathBuf>,
    pub warnings: Vec<String>,
}
/// The account running Alcuin. Another account can write a shared ancestor of the working
/// directory, such as /tmp or a drive root, so project files come only from what this account owns.
struct Account {
    /// The account's own home, which a changed HOME or USERPROFILE does not move.
    home: Option<PathBuf>,
    /// The process token's TOKEN_USER, in a buffer aligned for it.
    #[cfg(windows)]
    user: Option<Vec<u64>>,
}
impl Account {
    #[cfg(unix)]
    fn current() -> Self {
        use std::os::unix::ffi::OsStrExt;
        let mut buffer = vec![0 as libc::c_char; 16 * 1024];
        // SAFETY: an all-zero passwd is a valid out-parameter for getpwuid_r.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found = std::ptr::null_mut();
        // SAFETY: entry, buffer and found outlive the call; pw_dir points into buffer, which
        // outlives the copy taken here.
        let home = unsafe {
            (libc::getpwuid_r(
                libc::geteuid(),
                &mut entry,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut found,
            ) == 0
                && !found.is_null()
                && !entry.pw_dir.is_null())
            .then(|| {
                PathBuf::from(std::ffi::OsStr::from_bytes(
                    std::ffi::CStr::from_ptr(entry.pw_dir).to_bytes(),
                ))
            })
        };
        Self { home }
    }
    #[cfg(windows)]
    fn current() -> Self {
        use std::os::windows::ffi::OsStringExt;
        use windows_sys::Win32::{
            Foundation::{CloseHandle, HANDLE},
            Security::{GetTokenInformation, TOKEN_QUERY, TokenUser},
            System::Threading::{GetCurrentProcess, OpenProcessToken},
            UI::Shell::GetUserProfileDirectoryW,
        };
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: the pseudo-handle needs no closing; token is a valid out-pointer.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Self {
                home: None,
                user: None,
            };
        }
        let mut user = vec![0u64; 64];
        let mut size = 0u32;
        // SAFETY: the buffer holds the 512 bytes passed, enough for TOKEN_USER and any SID.
        let user = (unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                user.as_mut_ptr().cast(),
                (user.len() * 8) as u32,
                &mut size,
            )
        } != 0)
            .then_some(user);
        let mut home = vec![0u16; 1024];
        let mut length = home.len() as u32;
        // SAFETY: length gives the buffer's capacity in UTF-16 units; on success it holds the
        // count written, including the terminating zero.
        let home = (unsafe { GetUserProfileDirectoryW(token, home.as_mut_ptr(), &mut length) }
            != 0)
            .then(|| {
                let end = home
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(home.len());
                PathBuf::from(std::ffi::OsString::from_wide(&home[..end]))
            });
        // SAFETY: token came from OpenProcessToken above and is closed once.
        unsafe { CloseHandle(token) };
        Self { home, user }
    }
    #[cfg(unix)]
    fn owns(&self, file: &fs::File) -> bool {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        file.metadata()
            .is_ok_and(|metadata| metadata.uid() == unsafe { libc::geteuid() })
    }
    /// Owned by this account's user, or by the Administrators group when this process is an
    /// elevated administrator, whose new files Windows gives that group (as Git for Windows treats
    /// them).
    #[cfg(windows)]
    fn owns(&self, file: &fs::File) -> bool {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{ERROR_SUCCESS, LocalFree},
            Security::{
                Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
                CheckTokenMembership, EqualSid, IsWellKnownSid, OWNER_SECURITY_INFORMATION,
                TOKEN_USER, WinBuiltinAdministratorsSid,
            },
        };
        let Some(user) = &self.user else {
            return false;
        };
        let (mut owner, mut descriptor) = (std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: the handle is open for the call and the out-pointers are valid locals.
        if unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor,
            )
        } != ERROR_SUCCESS
        {
            return false;
        }
        let mut member = 0;
        // SAFETY: user holds the TOKEN_USER that GetTokenInformation wrote; owner points into
        // descriptor, which is freed once, after its last use. A volume without security, such
        // as FAT, reports no owner, which counts as not owned.
        unsafe {
            let sid = (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid;
            let owned = !owner.is_null()
                && (EqualSid(owner, sid) != 0
                    || (IsWellKnownSid(owner, WinBuiltinAdministratorsSid) != 0
                        && CheckTokenMembership(std::ptr::null_mut(), owner, &mut member) != 0
                        && member != 0));
            LocalFree(descriptor);
            owned
        }
    }
    fn owns_directory(&self, directory: &Path) -> bool {
        #[cfg(unix)]
        let opened = fs::File::open(directory);
        #[cfg(windows)]
        let opened = {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, READ_CONTROL,
            };
            fs::OpenOptions::new()
                .access_mode(READ_CONTROL)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(directory)
        };
        opened.is_ok_and(|directory| self.owns(&directory))
    }
    /// Each directory's project file, nearest first: from the working directory up to, but not
    /// including, the user's home or the first directory this account does not own.
    fn project_candidates(&self, cwd: &Path, user_home: Option<&Path>) -> Vec<PathBuf> {
        cwd.ancestors()
            .take_while(|directory| {
                Some(*directory) != user_home
                    && Some(*directory) != self.home.as_deref()
                    && self.owns_directory(directory)
            })
            .map(|directory| directory.join(".oracle/config.json"))
            .collect()
    }
}
pub fn load(
    home: &Path,
    cwd: &Path,
    user_home: Option<&Path>,
    include_project: bool,
) -> LoadedConfig {
    let user = home.join("config.json");
    let mut paths = vec![(user.clone(), false)];
    let account = Account::current();
    if include_project {
        let mut projects: Vec<_> = account
            .project_candidates(cwd, user_home)
            .into_iter()
            .filter(|path| *path != user)
            .map(|path| (path, true))
            .collect();
        projects.reverse();
        paths.extend(projects);
    }
    let mut result = LoadedConfig {
        config: json!({}),
        paths: vec![],
        warnings: vec![],
    };
    for (path, project) in paths {
        // Reference discoverProjectConfigPaths: only regular files are project candidates.
        if project {
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    result.warnings.push(format!(
                        "Failed to inspect {}: configuration is unreadable",
                        path.display()
                    ));
                    continue;
                }
            }
        }
        // A project file is read through the handle whose owner was checked, so it cannot be
        // swapped for another account's file between the check and the read.
        let read = if project {
            match fs::File::open(&path) {
                Ok(file) if !account.owns(&file) => {
                    result.warnings.push(format!(
                        "Failed to read {}: owned by another account",
                        path.display()
                    ));
                    continue;
                }
                opened => opened.and_then(|mut file| {
                    let mut raw = String::new();
                    std::io::Read::read_to_string(&mut file, &mut raw).map(|_| raw)
                }),
            }
        } else {
            fs::read_to_string(&path)
        };
        match read {
            Ok(raw) => match json5::from_str::<Value>(&raw) {
                Ok(value) => {
                    if value.is_object() {
                        result.config = deep_merge(
                            &result.config,
                            &if project {
                                sanitise_project(&value)
                            } else {
                                value
                            },
                        );
                        result.paths.push(path);
                    } else if !value.is_null() {
                        result.warnings.push(format!(
                            "Failed to read {}: config must be an object",
                            path.display()
                        ));
                    }
                }
                Err(_) => result.warnings.push(format!(
                    "Failed to read {}: invalid JSON5 configuration",
                    path.display()
                )),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => result.warnings.push(format!(
                "Failed to read {}: configuration is unreadable",
                path.display()
            )),
        }
    }
    result
}
pub fn resolve_engine(request: Option<&str>, config: &Value, env: &Environment) -> Result<String> {
    let environment = env
        .get("ORACLE_ENGINE")
        .map(|value| crate::policy::trim(value).to_lowercase());
    let environment = environment
        .as_deref()
        .filter(|value| matches!(*value, "api" | "browser"));
    let value = request
        .or(environment)
        .or(config["engine"].as_str())
        .unwrap_or_else(|| {
            if ["OPENAI_API_KEY", "OPENROUTER_API_KEY"]
                .iter()
                .any(|key| env.get(*key).is_some_and(|value| !value.is_empty()))
            {
                "api"
            } else {
                "browser"
            }
        });
    anyhow::ensure!(
        matches!(value, "api" | "browser"),
        "Engine must be api or browser."
    );
    Ok(value.to_owned())
}

/// Canonicalise the existing prefix without accepting dangling symlinks.
pub fn resolve_existing_prefix(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut prefix = absolute.as_path();
    let mut tail = vec![];
    loop {
        match fs::canonicalize(prefix) {
            Ok(mut resolved) => {
                for segment in tail.iter().rev() {
                    resolved.push(segment);
                }
                let mut normalised = PathBuf::new();
                for component in resolved.components() {
                    match component {
                        std::path::Component::CurDir => {}
                        std::path::Component::ParentDir => {
                            normalised.pop();
                        }
                        _ => normalised.push(component),
                    }
                }
                return Ok(normalised);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if fs::symlink_metadata(prefix).is_ok_and(|m| m.is_symlink()) {
                    anyhow::bail!(
                        "MCP output path contains an unresolved symlink: {}",
                        prefix.display()
                    );
                }
                tail.push(
                    prefix
                        .file_name()
                        .context("Unable to resolve output path")?
                        .to_owned(),
                );
                prefix = prefix.parent().context("Unable to resolve output parent")?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_account_owns_what_it_creates_and_not_system_files() {
        let account = Account::current();
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("PUBLIC.json");
        fs::write(&file, "{}").unwrap();
        assert!(account.owns(&fs::File::open(&file).unwrap()));
        assert!(account.owns_directory(root.path()));
        #[cfg(unix)]
        let (system_file, system_directory) = (PathBuf::from("/etc/passwd"), PathBuf::from("/"));
        #[cfg(windows)]
        let (system_file, system_directory) = {
            let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
            (system.join("kernel32.dll"), system)
        };
        #[cfg(unix)]
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return; // Root owns the system files too.
        }
        assert!(!account.owns(&fs::File::open(&system_file).unwrap()));
        assert!(!account.owns_directory(&system_directory));
    }
    #[test]
    fn the_walk_stops_before_the_accounts_own_home() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = home.join("project");
        fs::create_dir_all(&cwd).unwrap();
        let mut account = Account::current();
        account.home = Some(home);
        assert_eq!(
            account.project_candidates(&cwd, None),
            [cwd.join(".oracle/config.json")]
        );
        assert!(account.project_candidates(&cwd, Some(&cwd)).is_empty());
    }
    /// A working directory reached through a link to the home stops there too.
    #[cfg(unix)]
    #[test]
    fn the_walk_stops_at_the_home_under_another_name() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir_all(home.join("project")).unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&home, &alias).unwrap();
        let cwd = alias.join("project");
        let mut account = Account::current();
        account.home = Some(home.clone());
        assert_eq!(
            account.project_candidates(&cwd, None),
            [cwd.join(".oracle/config.json")]
        );
        account.home = None;
        assert_eq!(
            account.project_candidates(&cwd, Some(&home)),
            [cwd.join(".oracle/config.json")]
        );
    }
}
