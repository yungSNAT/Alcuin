# Configuration

Every setting can be passed as a flag. To keep one for every run, put it in a config file instead.

## Where settings come from

Alcuin reads 2 kinds of config file, both [JSON5](https://json5.org/) (comments and trailing commas are allowed):

- **User config:** `~/.oracle/config.json`, or `config.json` in `ORACLE_HOME_DIR` when that is set.
- **Project config:** `.oracle/config.json` in the current directory and in each parent directory up to, but not including, your home directory.

A project file overrides the user file, and a nearer project file overrides a farther one. Objects such as `browser` merge key by key, so a project file that sets only `browser.thinkingTime` keeps the rest of your `browser` settings.

A flag always wins. For the engine, the file size limit and notifications, an environment variable comes next and then the config files:

| Setting | Flag | Environment variable | Config key |
| --- | --- | --- | --- |
| Engine | `--engine` | `ORACLE_ENGINE` | `engine` |
| File size limit | `--max-file-size-bytes` | `ORACLE_MAX_FILE_SIZE_BYTES` | `maxFileSizeBytes` |
| Desktop notification | `--notify`, `--no-notify` | `ORACLE_NOTIFY` | `notify.enabled` |
| Notification sound | `--notify-sound`, `--no-notify-sound` | `ORACLE_NOTIFY_SOUND` | `notify.sound` |

These take the config file before the environment: `browser.maxConcurrentTabs` before `ORACLE_BROWSER_MAX_CONCURRENT_TABS`, `browser.manualLoginProfileDir` before `ORACLE_BROWSER_PROFILE_DIR`, and `browser.remoteHost` and `browser.remoteToken` before `ORACLE_REMOTE_HOST` and `ORACLE_REMOTE_TOKEN` ([A browser on another machine](remote.md)).

With no engine set anywhere, Alcuin uses the API when `OPENAI_API_KEY` or `OPENROUTER_API_KEY` is set and the browser otherwise.

## An example

```json5
// ~/.oracle/config.json
{
  engine: "browser",
  browser: {
    thinkingTime: "extra-high", // or "pro", the default
    keepBrowser: true,
  },
  promptSuffix: "Answer in Australian English.",
}
```

`--dry-run` shows what a run would send without contacting a model, so you can check a config file's effect:

```sh
alcuin --dry-run json -p "Review main.rs please" --file src/main.rs
```

## Keys

| Key | What it sets | Flag |
| --- | --- | --- |
| `engine` | `api` or `browser` | `--engine` |
| `model` | Model for new runs: `gpt-6-pro` (browser default) or `gpt-6-astra` (API default) | `--model` |
| `promptSuffix` | Text added on a new line after every prompt | none |
| `maxFileSizeBytes` | Largest file `--file` accepts, in bytes (default 1 MB) | `--max-file-size-bytes` |
| `filesReport` | Show token use per attached file | `--files-report` |
| `heartbeatSeconds` | Seconds between progress updates; 0 turns them off (default 30) | `--heartbeat` |
| `background` | Responses API background mode for API runs | `--background`, `--no-background` |
| `apiBaseUrl` | OpenAI-compatible base URL for API runs | `--base-url` |
| `azure` | `endpoint`, `deployment` and `apiVersion` for Azure OpenAI | `--azure-endpoint`, `--azure-deployment`, `--azure-api-version` |
| `notify` | `enabled` and `sound`, both `true` or `false` | `--notify`, `--notify-sound` |
| `browser.thinkingTime` | `pro` (default) or `extra-high` | `--browser-thinking-time` |
| `browser.modelStrategy` | `select` (default), `current` or `ignore` | `--browser-model-strategy` |
| `browser.keepBrowser` | Leave Chrome open after the run | `--browser-keep-browser` |
| `browser.manualLogin` | Use the persistent profile you sign in to | `--browser-manual-login` |
| `browser.manualLoginProfileDir` | Where that profile lives | `--browser-manual-login-profile-dir` |
| `browser.chromePath` | Chrome or Chromium executable | `--browser-chrome-path` |
| `browser.chatgptUrl` | ChatGPT URL, such as a project's | `--chatgpt-url` |
| `browser.timeoutMs` | Longest a browser run may take, in milliseconds | `--browser-timeout` |
| `browser.maxConcurrentTabs` | Most ChatGPT tabs open at once in one profile | `--browser-max-concurrent-tabs` |
| `browser.archiveConversations` | `auto`, `always` or `never` | `--browser-archive` |
| `browser.endMarker` | Ask for an end line and save an answer without it as `partial` | `--browser-end-marker` |

[Browser runs](browser.md) explains the thinking-time, strategy and profile settings.

## What a project file may set

A project file comes with the code you are reviewing, so Alcuin reads only these keys from it and ignores the rest:

- `engine`, `model`, `search`, `maxFileSizeBytes`, `notify`, `heartbeatSeconds`, `filesReport`, `background` and `promptSuffix`;
- under `browser`: `attachRunning`, `timeoutMs`, `inputTimeoutMs`, `approvalWaitMs`, `attachmentTimeoutMs`, `profileLockTimeoutMs`, `maxConcurrentTabs`, `cookieSyncWaitMs`, `hideWindow`, `keepBrowser`, `modelStrategy`, `thinkingTime`, `researchMode`, `archiveConversations` and `manualLogin`;
- `browser.chatgptUrl`, only when it is a ChatGPT address.

Keys such as `browser.chromePath`, `apiBaseUrl` and `azure` are read from the user file only, so a cloned repository cannot point Alcuin at another program or endpoint.

## When a file cannot be used

A file that is not valid JSON5, cannot be read or is not an object is skipped. The CLI prints the reason on stderr and carries on with the other files:

```text
Failed to read /home/you/project/.oracle/config.json: invalid JSON5 configuration
```

`alcuin-mcp` refuses the request instead, with the same reason followed by 'Fix or remove that file, then retry.', so a run never goes ahead without the settings you meant it to use.

Some keys Oracle reads are for features this build leaves out. A config file that sets one stops the run with the reason, for example:

```text
config key sessionRetentionHours is not supported in this build: session pruning is not included; stored sessions are kept for recovery.
```

The keys are `sessionRetentionHours`, `browser.assistantRecheckDelayMs`, `browser.assistantRecheckTimeoutMs`, `browser.reuseChromeWaitMs` and the 3 `browser.autoReattach*Ms` keys. `browser.captureProviderNative: true` is refused too.

## Environment variables

Alcuin keeps Oracle's names, so existing setups carry over.

| Variable | Effect |
| --- | --- |
| `ORACLE_HOME_DIR` | Folder for sessions, profiles and the user config (default `~/.oracle`) |
| `ORACLE_ENGINE` | `api` or `browser`; any other value is ignored |
| `ORACLE_MAX_FILE_SIZE_BYTES` | File size limit, in bytes |
| `ORACLE_MIN_PROMPT_CHARS` | Shortest prompt an API run accepts (default 10; a browser run always needs 10) |
| `ORACLE_BROWSER_PROFILE_DIR` | Persistent browser profile folder |
| `ORACLE_BROWSER_MAX_CONCURRENT_TABS` | Most ChatGPT tabs open at once |
| `ORACLE_NOTIFY`, `ORACLE_NOTIFY_SOUND` | `1`, `true`, `yes` or `on` to turn on; `0`, `false`, `no` or `off` to turn off |
| `ORACLE_DISABLE_NOTIFICATIONS` | `1` turns notifications off |
| `CHROME_PATH` | Chrome or Chromium executable to use |
| `OPENAI_API_KEY`, `OPENAI_BASE_URL` | OpenAI API key and base URL |
| `AZURE_OPENAI_API_KEY`, `AZURE_OPENAI_ENDPOINT`, `AZURE_OPENAI_DEPLOYMENT`, `AZURE_OPENAI_API_VERSION` | Azure OpenAI |
| `ORACLE_REMOTE_HOST`, `ORACLE_REMOTE_TOKEN` | A remote `alcuin serve` and its access token |
