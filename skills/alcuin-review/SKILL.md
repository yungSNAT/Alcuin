---
name: alcuin-review
description: Review uncommitted changes, a commit or a pull request with a ChatGPT second opinion through the Alcuin CLI, then check every cited file and line before reporting findings by severity. Use when asked for an Alcuin review or an independent code review of a change, and `alcuin` is installed.
---

# Alcuin review

Sends a change and the files it touches to ChatGPT with the `alcuin` CLI, checks everything the answer cites, and reports the findings that hold, from P0 to P3. Change no code until the user approves.

## 1. Collect the change

Make a new temporary folder and save the reviewed version of the change there:

- No argument, the uncommitted changes: `git diff HEAD` into `change.diff`. Untracked files are not in that diff: list them with `git ls-files --others --exclude-standard` and add them too. The reviewed files are the working copies.
- A number, that pull request: `gh pr diff <number>` into `change.diff`. Take the head commit from `gh pr view <number> --json headRefOid`, and fetch it with `git fetch origin pull/<number>/head`.
- `commit:<sha>`: `git show <sha>` into `change.diff`. The head commit is `<sha>`.

For a pull request or a commit, write each changed file's version at the head commit with `git show <head>:<path>` into a `files/` folder inside the temporary folder, keeping its repository path, and attach and check those copies, not the working tree's.

**Leave out secrets.** Alcuin's scanner refuses only known token formats and private keys; a password or connection string in an ordinary file goes out unseen. Before anything is attached:

- drop files that hold credentials or environment values (`.env*`, `*.pem`, `*.key`, `*.p12`, and credential or secrets files) from the attachments, and their hunks from `change.diff` (for example `git diff HEAD -- . ':(exclude).env*'`);
- read `change.diff` and cut any hunk with a password, token, key or connection string in it.

Beside the diff write a short `context.md`: what the change is meant to do (from the pull request description or commit messages), what is out of scope, and the project conventions that apply.

## 2. Preview and ask

Everything attached goes to the provider. Quote each path, as `--file` reads globs and a leading `!` excludes. Preview what would go:

```sh
alcuin --engine browser --dry-run summary --files-report -p "<prompt>" --file "<folder>/change.diff" --file "<folder>/context.md" --file "<reviewed file>"
```

Show the user the provider and the file list, and wait for approval. Say which files and hunks you left out and why.

## 3. Send

Use this prompt, with the scope filled in:

> Review this change against context.md. For each finding give a severity (P0: breaks correctness, security or data; P1: a likely bug; P2: a risk or a missing test; P3: a minor point), the file and line, a short exact quote of that line, and the reason. Say 'no findings' if there are none. Leave out style-only points.

```sh
alcuin --engine browser --slug "review of <scope>" -p "<prompt>" --file "<folder>/change.diff" --file "<folder>/context.md" --file "<reviewed file>"
```

The slug takes 3 to 5 words. The run prints its session ID (`<id>`); `alcuin status` also lists it. A run can take many minutes; start it in the background if you can. If it fails after sending, don't send it again: the answer may still arrive. Run `alcuin session <id> --harvest --write-output answer.md` to read it.

## 4. Check the answer

1. Run `alcuin ground <id>`. It exits 1 when a cited line runs past a sent file's end, a cited file was not sent or a quote appears in no sent file. Exit 2 means the check did not run: fix the ID, or report every finding as unchecked.
2. Open every cited file and line yourself, in the reviewed version. Drop a finding whose citation does not hold, and say why.
3. Rate each severity again against the code: a P0 that is only a style point goes down; a P3 that is a real race goes up.

## 5. Report and wait

Give one table: severity, file and line, finding, and verdict (accepted, dismissed with the reason, or unverified). List the dropped findings under it. Make no change until the user says which findings to fix; then fix those and run the project's tests.
