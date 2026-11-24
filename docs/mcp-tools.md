# MCP tool reference

What each `alcuin-mcp` tool takes and returns. [MCP setup](mcp.md) covers installing the server. Every tool returns its result as structured content; the server shares sessions, settings and the browser profile with the CLI.

The server sees only sessions started in or below its working directory, unless `ORACLE_MCP_ALL_SESSIONS=1` is set. Files are resolved relative to that directory and must lie inside it, unless `ORACLE_MCP_ALLOW_EXTERNAL_INPUT=1` is set.

## consult

Runs an Alcuin session through the API or ChatGPT in the browser. Only `prompt` is required.

| Input | Type | Meaning |
| --- | --- | --- |
| `prompt` | string | The prompt to send |
| `files` | array of strings | Files, folders or glob patterns to attach, as the CLI's `--file` takes them |
| `engine` | `api` or `browser` | Without it, the config file or `ORACLE_ENGINE` decides, then `api` when `OPENAI_API_KEY` is set, otherwise `browser` |
| `model` | string | Model name, as `--model` takes it |
| `preset` | `chatgpt-pro-heavy` | Browser, `gpt-6-pro` and Pro thinking, unless other inputs override them |
| `slug` | string | A readable session ID, as `--slug` sets it |
| `waitForCompletion` | boolean | `false` starts a detached run and returns its `sessionId` at once; call `wait` for the answer. Defaults to `true` |
| `dryRun` | boolean | Preview the run without creating a session or opening the browser |
| `search` | boolean | API only: turn the provider's search tool on or off |
| `maxOutput` | integer | API only: the most output tokens, reasoning included, the run may produce |
| `maxCost` | number | API only: refuse the run before sending if its estimated worst-case cost is more than this many US dollars, as `--max-cost` does; a lower `maxCost` in the user's config still applies |
| `browserThinkingTime` | string | `pro` (the default) or `extra-high` ([Browser runs](browser.md#model-and-thinking-level)) |
| `browserModelStrategy` | `select`, `current` or `ignore` | As `--browser-model-strategy` |
| `browserModelLabel` | string | A ChatGPT model label to select instead of the one `model` maps to |
| `browserAttachments` | `auto`, `always` or `never` | As `--browser-attachments`; `never` refuses a run with files |
| `browserBundleFiles` | boolean | Upload every file in one bundle |
| `browserBundleFormat` | `auto`, `text` or `zip` | As `--browser-bundle-format` |
| `browserResearchMode` | `search` | Turn on ChatGPT's web search |
| `browserFollowUps` | array of strings | More prompts to send in the same conversation after the first answer |
| `browserArchive` | `auto`, `always` or `never` | As `--browser-archive` |
| `browserFallback` | `grok` or `none` | `grok` consults grok.com once if ChatGPT fails ([Browser runs](browser.md#grok)) |
| `browserKeepBrowser` | boolean | Leave the browser open afterwards |
| `browserEndMarker` | boolean | As `--browser-end-marker` ([Partial answers](sessions.md#partial-answers)) |
| `browserSlotWaitMs` | integer | As `--browser-slot-wait`, in milliseconds |

| Output | Meaning |
| --- | --- |
| `sessionId` | The session's ID |
| `status` | The session's status, such as `completed`, `partial`, `error` or `running` |
| `output` | The whole saved answer, or a dry run's preview; empty until an answer is saved |
| `log` | The last 4,000 UTF-16 units of the session log |
| `dryRun` | Whether this was a preview |
| `resolved` | The engine, model, files and other settings the run resolved to |
| `models` | Each model's status, timings and usage |
| `modelSelection` | The model the browser asked for and the label it verified |
| `thinkingSelection` | The thinking level the browser asked for and the label it verified |
| `artifacts` | Files the run saved, such as the transcript |
| `images` | Images the run saved |
| `warnings` | Problems that did not stop the run, such as a staging folder that could not be removed |
| `browserCleanup` | Present when the browser tab still needs recovery |
| `detached` | Whether the run continues in a detached worker |

A client that declares the MCP Tasks extension gets `consult` back as a task, to poll with `tasks/get` and stop with `tasks/cancel`.

## wait

Waits for a session to finish without polling. A timeout or a cancelled request never stops the session: call `wait` again to keep waiting.

| Input | Type | Meaning |
| --- | --- | --- |
| `id` | string, required | Session ID or slug |
| `timeoutMs` | integer | How long to wait; omit it to wait until the session ends, or pass `0` for its state now |

Its output has `sessionId`, `status`, `output`, `log`, `models`, `modelSelection`, `thinkingSelection`, `artifacts`, `images`, `warnings` and `browserCleanup`, as `consult` returns them, and:

| Output | Meaning |
| --- | --- |
| `waitStatus` | `terminal` when the session ended, `timed_out` when the wait did |
| `timedOut` | Whether the wait timed out |
| `cancelled` | Whether the request was cancelled |

## sessions

Lists recent sessions, or fetches one.

| Input | Type | Meaning |
| --- | --- | --- |
| `id` | string | Fetch this session (ID or slug) instead of listing |
| `detail` | boolean | With `id`: include the metadata, the stored request and the whole log |
| `hours` | number | Look back this many hours (24 by default) |
| `limit` | number | Most sessions to list (100 by default) |
| `includeAll` | boolean | List sessions of any age, as `alcuin status --all` does |

| Output | Meaning |
| --- | --- |
| `entries` | The listed sessions, with ID, creation time, status, model and mode |
| `total` | How many sessions matched |
| `truncated` | Whether `limit` cut the list short |
| `session` | With `id`: the session's metadata, log and request |

## ground

Checks a saved answer's citations and quotes against what its session sent, as [`alcuin ground --json`](sessions.md#check-an-answers-evidence) does. A flagged answer is a result, not an error.

| Input | Type | Meaning |
| --- | --- | --- |
| `id` | string, required | Session ID or slug |

| Output | Meaning |
| --- | --- |
| `sessionId` | The session checked |
| `flagged` | `true` when a citation runs past a file's end or names a file not sent, or a quote is found nowhere |
| `sentFiles` | `saved` when it read the copy the session kept of what it sent, `current` when it read the files as they are now |
| `references` | Each `path:line` citation, its answer line and its status |
| `quotes` | Each quote of 8 or more characters, its answer line and whether it was found |
| `changedFiles` | With current files: those changed since the session was created |
| `skippedFiles` | Sent files that are not UTF-8 text, so nothing was checked against them |

## project_sources

Lists a ChatGPT Project's Sources tab, or previews which files an upload would add. Alcuin runs ChatGPT only in Temporary Chat, so it refuses the upload itself.

| Input | Type | Meaning |
| --- | --- | --- |
| `operation` | `list` or `add`, required | `add` needs `dryRun` set to `true` and only previews |
| `chatgptUrl` | string | The project's address, ending in `/project`; defaults to `browser.chatgptUrl` in the config |
| `files` | array of strings | Files or glob patterns to preview |
| `dryRun` | boolean | Check the files and return the plan without opening the browser |
| `confirmMutation` | boolean | Required for an `add` that would change the project |
| `browserKeepBrowser` | boolean | Leave the browser open afterwards |

| Output | Meaning |
| --- | --- |
| `status` | `ok` or `dry-run` |
| `operation` | The operation run |
| `projectUrl` | The project's address |
| `dryRun` | Whether this was a preview |
| `sourcesBefore` | The sources listed, each with name, position and status |
| `sourcesAfter` | The sources after an `add` |
| `plannedUploads` | The files an `add` would upload, with sizes |
| `added` | The sources an `add` added |
| `warnings` | Problems that did not stop the operation |
| `tookMs` | How long it took, in milliseconds |
