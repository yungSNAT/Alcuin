# Browser runs

`--engine browser` sends your prompt and files to ChatGPT in a Chromium-based browser that Alcuin drives. This guide covers the model and thinking level, how files are delivered, the browser profile and tabs, and the options for a run that needs more than one answer. [Quickstart](quickstart.md) covers signing in for the first time.

## Model and thinking level

Every new request asks for Latest (GPT-6). `--model` takes `gpt-6-pro` (the browser default), `gpt-6`, `gpt-6-astra` or `Latest`; any other model, such as `gpt-5.6`, is refused before anything is sent.

`--browser-thinking-time` takes `pro` (the default) or `extra-high`. Before it sends the prompt, Alcuin selects the model and thinking level in ChatGPT's picker and checks that both are shown as selected:

- A Pro request falls back to Extra High only when ChatGPT shows that Pro is unavailable. A missing or ambiguous control stops the run instead.
- An Extra High request never tries Pro.
- Any other level, such as `heavy` or `light`, is refused before anything is sent. A `--dry-run` preview does not check the level.

`--browser-model-strategy` sets how the picker is used: `select` (the default) chooses the requested model, `current` keeps the model already selected and `ignore` leaves the picker alone. The label ChatGPT shows does not prove which backend model answered.

`alcuin browser-health` opens a fresh tab and reports whether you are signed in and which model and thinking level are selected, without selecting anything or sending a prompt. Add `--json` for the result as JSON.

## Temporary Chat

Each new request opens a Temporary Chat. ChatGPT offers neither Deep Research nor image generation in Temporary Chat, so `--browser-research deep`, `--generate-image` and `--output` are refused before anything is sent. `--browser-research search` turns on web search.

## Web addresses in the prompt

Alcuin counts a send as made only when the message ChatGPT sends carries exactly your prompt. ChatGPT turns a web address in the prompt, such as `https://example.com/a_b`, into a link and rewrites the prompt as escaped Markdown: `[https://example.com/a\_b](https://example.com/a\_b)`. Alcuin accepts that form only when decoding it gives your prompt exactly and each link's text and target are the address you typed. Any other difference leaves the send uncertain, with 'commitment is ambiguous (submission-witness-unavailable: unqualified-content)', and the prompt may still have reached ChatGPT: recover the session rather than sending it again ([Troubleshooting](troubleshooting.md#a-run-stopped-or-timed-out)).

With `--browser-research search`, Alcuin does not decode the prompt, so a send whose prompt holds a web address stays uncertain. Put such addresses in an attached file instead.

## Files

`--file` takes files, folders and glob patterns; prefix a pattern with `!` to leave matching files out. Alcuin uploads the files as attachments rather than pasting them into the prompt:

| Option | Effect |
| --- | --- |
| `--browser-attachments auto` | The default: upload the files |
| `--browser-attachments always` | Upload the files |
| `--browser-attachments never` | Refuse the run if it has files, as pasted source cannot be checked |
| `--browser-bundle-files` | Upload every file in one bundle |
| `--browser-bundle-format auto`, `text` or `zip` | Bundle text files as one flattened text file, and use a ZIP when other files are present (`auto`), or force either |

Several text files are bundled into one upload by default. Each file must be under the size limit, 1 MB unless you set `--max-file-size-bytes`, `ORACLE_MAX_FILE_SIZE_BYTES` or `maxFileSizeBytes` ([Configuration](configuration.md)). A file that looks like it holds a credential is refused unless you name it with `--allow-secret-file <path>`.

Preview what a run would send, with each file's size, before sending it:

```sh
alcuin --engine browser --dry-run summary --files-report -p "Review the tree" --file "src/**/*.rs"
```

## The browser profile

Alcuin keeps its own ChatGPT sign-in in a profile at `~/.oracle/browser-profile`, separate from your everyday browser. `--browser-manual-login` uses that profile, so you can sign in once and later runs reuse it. On Windows runs use it by default; on Linux pass `--browser-manual-login`, or set `browser.manualLogin` to `true` in [your config](configuration.md). `--browser-manual-login-profile-dir <path>` or `ORACLE_BROWSER_PROFILE_DIR` uses another folder.

| Option | Effect |
| --- | --- |
| `--browser-keep-browser` | Leave the browser open after the run |
| `--browser-headless` | Run the browser without a window |
| `--browser-chrome-path <path>` | Use this browser executable; `CHROME_PATH` does the same |
| `--browser-max-concurrent-tabs <n>` | Most ChatGPT tabs open at once in one profile; further runs wait for a free tab |
| `--browser-tab <ref>` | Reuse an open ChatGPT tab, by `current`, target ID, URL or part of its title |
| `--browser-attach-running` | Use a browser you already started with remote debugging, instead of launching one |
| `--chatgpt-url <url>` | Open another ChatGPT address, such as a project's |
| `--browser-timeout <duration>` | Longest a run may wait for the answer |

`--browser-attach-running` cannot be combined with options that configure the browser Alcuin would otherwise launch, such as `--browser-manual-login`, `--browser-keep-browser` or `--browser-port`.

## More than one answer

- `--browser-follow-up <prompt>` sends another prompt in the same conversation after the first answer. Repeat it for more turns.
- `--followup <id>` continues a saved session later ([Sessions](sessions.md#continue-or-repeat-a-session)).
- `--browser-end-marker` asks ChatGPT to finish with a line unique to the run and saves an answer without it as `partial` ([Partial answers](sessions.md#partial-answers)). It needs one turn and a local browser.
- `--browser-archive auto`, `always` or `never` archives the ChatGPT conversation once Alcuin has saved the answer. `auto` archives only successful single-turn runs outside a project.

## Grok

`--browser-fallback grok` consults grok.com once, in its own sign-in profile, after ChatGPT fails and Alcuin's read-only recovery of the ChatGPT answer has run out. `--browser-provider grok` asks Grok directly without opening ChatGPT. Grok takes text prompts only: a run that uploads files never falls back to Grok, and `--browser-provider grok` refuses `--file`.
