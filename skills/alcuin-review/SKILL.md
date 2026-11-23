---
name: alcuin-review
description: Review uncommitted changes, a commit or a pull request with a ChatGPT second opinion through the Alcuin CLI, then check every cited file and line before reporting findings by severity. Use when asked for an Alcuin review or an independent code review of a change, and `alcuin` is installed.
---

# Alcuin review

Sends a change and the files it touches to ChatGPT with the `alcuin` CLI, checks everything the answer cites, and reports the findings that hold, from P0 to P3. Change no code until the user approves.

## 1. Choose the change

- No argument: the uncommitted changes, `git diff HEAD`.
- A number: that pull request, `gh pr diff <number>`.
- `commit:<sha>`: that commit, `git show <sha>`.

Make a new temporary folder. Save the diff there as `change.diff`, and beside it a short `context.md`: what the change is meant to do (from the pull request description or commit messages), what is out of scope, and the project conventions that apply.

## 2. Preview and ask

Everything attached goes to the provider. Attach `change.diff`, `context.md` and the changed files that still exist, and preview what would go:

```sh
alcuin --dry-run summary --files-report -p "<prompt>" --file <folder>/change.diff --file <folder>/context.md --file <changed files>
```

Show the user the provider and the file list, and wait for approval. Never attach secrets: Alcuin refuses a file or prompt that matches a credential pattern.

## 3. Send

Use this prompt, with the scope filled in:

> Review this change against context.md. For each finding give a severity (P0: breaks correctness, security or data; P1: a likely bug; P2: a risk or a missing test; P3: a minor point), the file and line, a short exact quote of that line, and the reason. Say 'no findings' if there are none. Leave out style-only points.

```sh
alcuin --engine browser --slug "review of <scope>" -p "<prompt>" --file <folder>/change.diff --file <folder>/context.md --file <changed files>
```

A run can take many minutes; start it in the background if you can. If it fails after sending, don't send it again: the answer may still arrive. Run `alcuin session <id> --harvest --write-output answer.md` to read it.

## 4. Check the answer

1. Run `alcuin ground <id>`. It exits 1 when a cited line runs past a sent file's end, a cited file was not sent or a quote appears in no sent file.
2. Open every cited file and line yourself. Drop a finding whose citation does not hold, and say why.
3. Rate each severity again against the code: a P0 that is only a style point goes down; a P3 that is a real race goes up.

## 5. Report and wait

Give one table: severity, file and line, finding, and verdict (accepted, dismissed with the reason, or unverified). List the dropped findings under it. Make no change until the user says which findings to fix; then fix those and run the project's tests.
