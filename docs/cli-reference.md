# CLI reference

Every command and every option that `alcuin --help` and each command's `--help` show. `alcuin --help --verbose` adds the search, token and browser overrides, and `alcuin --debug-help` the advanced and debug options. The guides explain the options in context: [Browser runs](browser.md), [Files and limits](files.md), [Configuration](configuration.md) and [Sessions](sessions.md).

```sh
alcuin [options] [prompt]
alcuin <command> [options]
```

A bare argument is the prompt, as `-p` gives it.

## Prompt and files

| Option | What it does |
| --- | --- |
| `-p`, `--prompt <text>` | The prompt to send |
| `-f`, `--file <paths...>` | Files, folders or glob patterns to attach; prefix a pattern with `!` to leave matches out |
| `--allow-secret-file <paths...>` | Send these files, each named by exact path, despite a credential match, for this run only |
| `--max-file-size-bytes <bytes>` | Refuse files larger than this (1 MB by default) |
| `--files-report` | Show each attached file's token count in an API run |
| `--dry-run [mode]` | Preview without contacting a model: `summary` (the default), `json` or `full` |
| `--render-markdown`, `--render` | Print the assembled prompt and files as Markdown and exit |
| `--render-plain` | Print that Markdown without colour or highlighting |
| `--copy-markdown` | Copy the assembled Markdown to the clipboard |
| `-s`, `--slug <words>` | Name the session with 3 to 5 words |

## Engine and model

| Option | What it does |
| --- | --- |
| `-e`, `--engine <mode>` | `api` or `browser`; without it, `api` when `OPENAI_API_KEY` is set, otherwise `browser` |
| `-m`, `--model <model>` | Latest (GPT-6) only: `gpt-6-pro` in the browser and `gpt-6-astra` through the API by default; older models are refused |
| `--models <models>` | A comma-separated list of API models for one multi-model run |
| `--reasoning-effort <effort>` | API reasoning effort: `none`, `low`, `medium`, `high`, `xhigh` or `max` |
| `--reasoning-mode <mode>` | API reasoning mode; new requests need `pro` |

## Running and waiting

| Option | What it does |
| --- | --- |
| `--wait` | Stay attached until the answer is saved, instead of returning once a detached run starts |
| `--force` | Start a new session even if the same prompt is already running |
| `--followup <sessionId\|responseId>` | Continue a saved browser conversation or API response |
| `--followup-model <model>` | For a multi-model API session, which model's response to continue |
| `--timeout <seconds\|duration\|auto>` | Overall limit for an API call (`auto`: 60 minutes for Pro models, 120 seconds otherwise) |
| `--http-timeout <ms\|s\|m\|h>` | HTTP client timeout for API requests (20 minutes by default) |
| `--background`, `--no-background` | Use or skip the Responses API's background mode for API runs |
| `--store` | Keep an API run's response at OpenAI for at least 30 days, so `--followup` can continue it; API runs are not kept otherwise |
| `--heartbeat <seconds>` | Print a progress line this often (30 by default; 0 turns it off) |
| `--zombie-timeout <ms\|s\|m\|h>` | Age after which `alcuin status` treats a running session as stale (60 minutes by default) |
| `--zombie-last-activity` | Measure that age from the last log activity rather than the start |
| `--notify`, `--no-notify` | Turn the desktop notification on or off ([Notifications](notifications.md)) |
| `--notify-sound`, `--no-notify-sound` | Play a sound with it, or not (off by default) |
| `-v`, `--verbose` | Log every step |
| `--perf-trace` | Write a timing trace as JSON (or set `ORACLE_PERF_TRACE`) |
| `--perf-trace-path <path>` | Write that trace to this path |
| `--debug-help` | Show the advanced and debug options and exit |

## Output

| Option | What it does |
| --- | --- |
| `--write-output <path>` | Write only the final answer to this file, overwriting it |
| `--write-artifacts` | Also save files the browser run captured, beside `--write-output`, without overwriting |
| `--allow-partial` | Exit 0 for a multi-model run when at least one model succeeds |
| `--partial <mode>` | Multi-model failure policy: `fail` or `ok` |

## API provider

| Option | What it does |
| --- | --- |
| `--route` | Print the API provider route and exit |
| `--preflight` | Check that the API provider is ready for the model and exit |
| `--base-url <url>` | Another OpenAI API base URL on `api.openai.com` or a subdomain of it; an API run through any other URL, such as a proxy, is refused before it is sent, and `--route`, `--preflight` and `--dry-run` refuse it too |
| `--provider <provider>` | `auto` (the default), `openai` or `azure` |
| `--no-azure` | Ignore Azure settings for this run, as `--provider openai` does |
| `--azure-endpoint <url>` | Azure OpenAI endpoint |
| `--azure-deployment <name>` | Azure OpenAI deployment name |
| `--azure-api-version <version>` | Azure OpenAI API version |
| `--max-cost <dollars>` | Refuse an API run whose estimated worst-case cost is more than this many US dollars ([Configuration](configuration.md#a-spend-cap)); shown by `--help --verbose` |

## Browser

| Option | What it does |
| --- | --- |
| `--browser-model-strategy <mode>` | `select` (the default) picks the requested model, `current` keeps the selected one, `ignore` leaves the picker alone |
| `--browser-research <mode>` | `off`, or `search` for web search; `deep` is refused ([Known limitations](../README.md#known-limitations)) |
| `--browser-attachments <mode>` | `auto` (the default) and `always` upload files; `never` refuses a run with files |
| `--browser-inline-files` | The same as `--browser-attachments never` |
| `--browser-bundle-files` | Upload every file in one bundle |
| `--browser-bundle-format <format>` | `auto` (the default), `text` or `zip` |
| `--browser-follow-up <prompt>` | Send another prompt in the same conversation after the answer; repeat for more |
| `--browser-end-marker` | Ask for a line unique to the run and save an answer without it as `partial` |
| `--browser-archive <mode>` | Archive the ChatGPT conversation once the answer is saved: `auto`, `always` or `never` |
| `--browser-fallback <provider>` | `grok` consults grok.com once if ChatGPT fails; `none` does not |
| `--browser-provider <provider>` | `chatgpt` (the default), or `grok` to ask Grok directly, text only |
| `--chatgpt-url <url>` | Open another ChatGPT address, such as a project's |
| `--browser-tab <ref>` | Reuse an open ChatGPT tab: `current`, a target ID, a URL or part of its title |
| `--browser-attach-running` | Use a browser already running with remote debugging (127.0.0.1:9222 unless `--remote-chrome` says otherwise) |
| `--remote-chrome <host:port>` | Connect to Chrome's DevTools protocol at this address |
| `--browser-port <port>` | Use a fixed DevTools port |
| `--browser-approval-wait <duration>` | How long to wait at each remote-debugging approval prompt (20 seconds by default) |
| `--copy-profile <dir>` | Run against a throwaway copy of a signed-in browser profile, removed afterwards |
| `--browser-cookie-sync` | Copy cookies from your running browser (off by default; not on Windows) |
| `--browser-cookie-path <path>` | The cookie database to copy them from |
| `--remote-host <host:port>` | Send browser runs to an `alcuin serve` host ([A browser on another machine](remote.md)) |
| `--remote-token <token>` | That host's access token |
| `--generate-image <file>`, `--edit-image <file>`, `--output <file>` | Image generation and editing; refused, as ChatGPT offers neither in Temporary Chat |

`-V`, `--version` prints the version and `-h`, `--help` the help.

## Commands

### alcuin status

Lists recent sessions, or attaches to one when given an ID.

| Option | What it does |
| --- | --- |
| `--hours <hours>` | Look back this many hours (24 by default) |
| `--limit <count>` | Show at most this many sessions (100 by default, 1,000 at most) |
| `--all` | Include sessions of any age |
| `--model <name>` | Only this model's sessions |
| `--render`, `--render-markdown` | Render a completed answer as Markdown |
| `--hide-prompt` | Leave out the stored prompt |
| `--browser-tabs` | List open ChatGPT tabs and the sessions they belong to |

### alcuin session

Attaches to a session, or lists recent ones without an ID. It takes `--hours`, `--limit`, `--all`, `--model`, `--render`, `--render-markdown` and `--hide-prompt` as `status` does, and:

| Option | What it does |
| --- | --- |
| `--path` | Print the session's stored paths instead of attaching |
| `--harvest` | Re-read the session's browser tab and save the latest answer |
| `--live` | Follow the tab until the answer completes, stalls or detaches |
| `--write-output <path>` | Write what `--harvest` or `--live` read to this file |
| `--browser-tab <ref>` | The tab to read, instead of the one saved |
| `--no-recover` | Do not reopen the saved conversation when no tab is open |

### alcuin restart

Runs a saved session again as a new session with the same options.

| Option | What it does |
| --- | --- |
| `--wait` | Wait in this terminal instead of detaching |
| `--remote-host <host:port>`, `--remote-token <token>` | Send it to an `alcuin serve` host |

### alcuin ground

Flags cited lines and quotes in a saved answer that the session's files do not contain ([Sessions](sessions.md#check-an-answers-evidence)). `--json` prints every citation and quote with its status.

### alcuin browser-health

Opens a fresh tab and reports the sign-in, model and thinking level without selecting or sending anything. `--json` prints the result as JSON.

### alcuin recover-native

Saves a completed ChatGPT reply that matches a saved request.

| Option | What it does |
| --- | --- |
| `--request <path>` | Request JSON with `chatId`, `label` and an optional `turnId` |
| `--write-output <path>` | The new Markdown file for the answer; an existing file is kept |

### alcuin serve

Runs Alcuin's browser as a service for other machines ([A browser on another machine](remote.md)).

| Option | What it does |
| --- | --- |
| `--host <address>` | Interface to listen on (127.0.0.1 by default); traffic is plain HTTP |
| `--port <number>` | Port (random by default) |
| `--token <value>` | Access token (random by default); visible in process listings |
| `--token-file <path>` | Read the token from a file instead |
| `--max-concurrent-runs <count>` | Allow this many runs at once, queueing the rest |
| `--max-queued-runs <count>` | Most waiting requests in that mode (8 by default) |
| `--manual-login` | Use the dedicated sign-in profile |
| `--manual-login-profile-dir <path>` | That profile's folder |
| `--browser-cookie-sync` | Copy cookies from this machine's browser instead |

### alcuin recover-remote

Retrieves a run saved on an `alcuin serve` host into a new local session without sending the prompt again. Takes the run ID, `--host <host:port>` and `--token <value>`.

### alcuin bridge

Connects a browser on one machine to clients on another ([A browser on another machine](remote.md)).

| Command | Options |
| --- | --- |
| `alcuin bridge host` | `--bind <host:port>`, `--token <token\|auto>`, `--write-connection <path>`, `--ssh <user@host>`, `--ssh-remote-port <port>`, `--ssh-identity <path>`, `--ssh-extra-args <args>`, `--background`, `--foreground`, `--print`, `--print-token` |
| `alcuin bridge client` | `--connect <connection>`, `--config <path>`, `--no-write-config`, `--no-test`, `--print-env` |
| `alcuin bridge doctor` | `--verbose`: checks the browser, config and remote host |
| `alcuin bridge codex-config` | `--local-browser`, `--print-token`, `--oracle-home-dir <path>`, `--browser-profile-dir <path>`: prints a Codex MCP entry |
| `alcuin bridge claude-config` | The same options: prints a Claude Code MCP entry ([MCP setup](mcp.md)) |

### alcuin project-sources

Lists a ChatGPT Project's sources (`alcuin project-sources list`) or previews an upload (`alcuin project-sources add` with `--file <paths...>` and `--dry-run`); uploads themselves are refused. Both take `--chatgpt-url <url>`, `--json`, `-v`, `--verbose` and `--max-file-size-bytes <bytes>`, and these browser options: `--browser-manual-login`, `--browser-manual-login-profile-dir <path>`, `--browser-timeout <duration>`, `--browser-input-timeout <duration>`, `--browser-profile-lock-timeout <duration>`, `--browser-max-concurrent-tabs <n>`, `--browser-cookie-wait <duration>`, `--browser-chrome-profile <profile>`, `--browser-chrome-path <path>`, `--browser-cookie-path <path>`, `--browser-inline-cookies <json>`, `--browser-inline-cookies-file <path>`, `--browser-cookie-sync`, `--browser-no-cookie-sync`, `--browser-keep-browser`, `--browser-hide-window` (macOS only) and `--browser-allow-cookie-errors`.

### alcuin doctor

Checks API provider readiness and routing. Takes `--providers`, `--models <models>`, `-m`, `--model <model>`, `--provider <provider>`, `--no-azure`, `--azure-endpoint <url>`, `--azure-deployment <name>`, `--azure-api-version <version>`, `--base-url <url>` and `--json`.

### alcuin perf-summary

Summarises the timings in trace files written with `--perf-trace`. `--json` prints them as JSON.

### alcuin docs check

Checks Markdown files' `--` options against this CLI. `--docs-path <file>...` names the files and `--json` prints the result as JSON; it exits 1 on drift.

### alcuin tui

Not supported in this build ([Known limitations](../README.md#known-limitations)).
