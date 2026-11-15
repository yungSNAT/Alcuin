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
| `maxCost` | Refuse an API run whose estimated worst-case cost is more than this many US dollars ([A spend cap](#a-spend-cap)) | `--max-cost` |
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

## A spend cap

`maxCost`, or `--max-cost <dollars>` for one run, refuses an API run before anything is sent when its estimated worst-case cost is more than that many US dollars. Alcuin estimates the most the run could cost: its input, by Alcuin's own token count, at the highest rate input is billed at (a cache write, 1.25 times the input rate), plus `--max-output` tokens, which include reasoning, at the output rate. So a capped run needs `--max-output` and `--search off`, as search adds charges Alcuin cannot count, and a follow-up is refused, as its earlier conversation is billed again without being counted:

```sh
alcuin --engine api --max-cost 1 --max-output 30000 --search off -p "Review this module" --file src/lib.rs
```

Here 30,000 output tokens alone could cost $1.50, so the run is refused, exit code 1, with 'This run's estimated worst-case cost is $1.5…, over --max-cost $1. Lower --max-output or attach less.'

The input count is Alcuin's own estimate (the o200k_base tokenizer plus a 200-token allowance), and the rates are GPT-6 Astra's standard OpenAI prices that Alcuin bundles ($10 input and $50 output per 1M tokens, read 15 November 2026). The cap is on this estimate: the provider's own count can differ, and an Azure deployment is priced at the same rates, so leave headroom, or lower the cap if your Azure rates are higher.

When both are set, the lower cap applies, so a run or an MCP client can tighten the cap in your config but not lift it. A project file cannot set it. Browser runs are not affected.

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
