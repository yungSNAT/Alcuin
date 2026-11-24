# Coding agents

A coding agent can use Alcuin to get a second opinion from another model on the files it chooses. It can call the CLI, taught by the `alcuin` skill, or the MCP server's tools ([MCP setup](mcp.md)). Either way, the agent runs on your machine with your sign-in, so you decide what it may send.

## Add the skill

The skill tells an agent when to consult, how to preview and send, how to read and check the answer, and never to send a prompt twice. Add it for every agent the [Skills CLI](https://github.com/vercel-labs/skills) knows, for your user:

```sh
npx skills add yungSNAT/Alcuin --skill alcuin --global --agent "*" --yes
```

It lands in `~/.agents/skills/alcuin`, linked into each agent's own skills folder. For one project only, drop the global option: the skill then goes into the current project's `.agents/skills/alcuin`, for the agents you name:

```sh
npx skills add yungSNAT/Alcuin --skill alcuin --agent claude-code codex --yes
```

For the review skill described below, run either command with `alcuin-review` in place of the skill name `alcuin`.

The [Claude Code plugin](mcp.md#claude-code-plugin) adds the same skill together with the MCP server, and an `alcuin-review` skill. That one reviews uncommitted changes, a commit or a pull request: it previews and asks before sending, runs `ground` on the answer, opens every cited line, and reports the findings that hold from P0 to P3 without changing code until you approve. For Codex, one `codex mcp add` command adds the server ([Codex in one command](mcp.md#codex-in-one-command)).

## What the skill asks of the agent

- **Approval before sending.** Everything attached goes to the provider, so the agent shows you the provider and the files and waits for your approval. Alcuin refuses a file or prompt that matches a credential pattern ([Files and limits](files.md#checks-before-sending)).
- **A preview first.** It checks what would be sent, without contacting a model:

  ```sh
  alcuin --dry-run summary --files-report -p "Review this module" --file src/main.rs
  ```

- **You sign in.** The first browser run opens a window where you sign in to ChatGPT; the agent cannot do that for you ([Quickstart](quickstart.md#3-sign-in-once)).
- **No second send.** A run can take many minutes, and a Pro run continues in a detached session. If the command returns or fails after sending, the agent attaches to the session instead of sending the prompt again; a second run of the same prompt while the first is going exits 1 ([Exit codes](exit-codes.md#a-run-already-going)).
- **Check before acting.** It reads the answer and runs `ground` on it before relying on what it cites:

  ```sh
  alcuin status
  alcuin session <id> --render
  alcuin ground <id>
  ```

  `ground` exits 1 when it flags a citation or quote, so the agent checks those against the source ([Sessions and checking answers](sessions.md#check-an-answers-evidence)).

## In your project's instructions

To make the rules part of a project, add a few lines to its `AGENTS.md` or `CLAUDE.md`, for example:

```markdown
## Second opinions
- Ask before sending any file to Alcuin, and say which provider it goes to.
- Preview with `--dry-run summary --files-report` first.
- Never rerun a prompt that may have been sent; attach to its session.
- Run `alcuin ground` on an answer before acting on its citations.
```

## Exit codes

An agent can tell the outcome from the exit code: 0 for success, 1 for an error, a refusal, a duplicate run or a `partial` answer, 2 when `ground` cannot check a session and 130 after Ctrl-C ([Exit codes and errors](exit-codes.md)).

<sub>Checked on 15 November 2026 with a build of `main` on Linux: both `npx skills add` commands in a scratch home, the preview and `status`. Reading and grounding a real session need a signed-in ChatGPT account and were not run.</sub>
