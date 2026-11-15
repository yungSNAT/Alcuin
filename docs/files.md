# Files and limits

Alcuin cannot see your project: the model reads only the prompt and the files you attach. This guide covers choosing files, the checks Alcuin makes before anything is sent and how to preview a run.

## Choose files

`--file` (or `-f`) takes files, folders and glob patterns, as many as you like. Prefix a pattern with `!` to leave matching files out:

```sh
alcuin -p "Review the parser" --file src "!src/**/*_tests.rs" README.md
```

Quote glob patterns so your shell passes them to Alcuin unexpanded. Paths are relative to the current directory.

When Alcuin expands a folder or pattern it skips:

- files and folders your `.gitignore` files exclude;
- folders named `node_modules`, `dist`, `coverage`, `.git`, `.turbo`, `.next`, `build` or `tmp`, unless you name the folder itself;
- files whose names start with a dot, unless your pattern names one.

## Checks before sending

Each check runs before anything leaves your machine, and a run that fails one sends nothing.

- **Size.** Each file must be no larger than 1 MB, except that a browser run uploads media files and archives as they are, with no limit unless you set one. `--max-file-size-bytes <bytes>`, `ORACLE_MAX_FILE_SIZE_BYTES` or `maxFileSizeBytes` in [your config](configuration.md) sets the limit for every file; the flag wins, then the environment variable, then the config.
- **Credential files.** Files named like credentials or private keys are never sent: `.env` and `.env.*` (except `.env.example`, `.env.sample`, `.env.template` and `.env.dist`), `.npmrc`, `.pypirc`, `.netrc`, `.git-credentials`, `.htpasswd`, SSH keys such as `id_ed25519`, and files ending `.pem`, `.key`, `.ppk`, `.p12`, `.pfx`, `.jks`, `.keystore` or `.kdbx`. Named directly, such a file stops the run; found in a folder or pattern, it is skipped with a notice on stderr.
- **Credential contents.** Every file is scanned for values that look like keys or tokens, and a match stops the run with the file and line. For a test fixture whose fake key trips the scan, `--allow-secret-file <path>` sends that one file anyway; it applies to the run it is given with, and `alcuin-mcp` offers no such override.
- **Unchanged since the scan.** A file that changes between the scan and the upload stops the run.

## Preview a run

`--dry-run` shows what a run would send without contacting a model. It takes `summary` (the default), `json` or `full`, which prints the whole assembled prompt:

```sh
alcuin --dry-run summary --files-report -p "Review the parser" --file src
```

`--files-report` adds each file's token count. It is shown anyway when the files exceed the input budget, which `--max-input <tokens>` changes.

`--render` prints the assembled prompt and files as one Markdown document and exits, and `--copy-markdown` copies it to the clipboard, so you can paste it into a chat yourself.

## How files reach the model

A browser run uploads the files as attachments, bundling several text files into one upload; [Browser runs](browser.md#files) lists the options. An API run puts the files' text into the request.
