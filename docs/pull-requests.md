# Review a pull request in GitHub Actions

This workflow asks the API engine to review each pull request's changed files and posts the answer as a comment. A hosted runner holds no ChatGPT sign-in, so it uses the API: add your OpenAI key as the repository secret `OPENAI_API_KEY`, and each review is billed to that key.

Save it as `.github/workflows/alcuin-review.yml`:

```yaml
name: Alcuin review

on:
  pull_request:

permissions:
  contents: read

jobs:
  review:
    # Pull requests from forks get no secrets, so only this repository's branches are reviewed.
    if: github.event.pull_request.head.repo.full_name == github.repository
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: write
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0
          persist-credentials: false
      - name: Install Alcuin
        run: |
          curl -fsSL https://raw.githubusercontent.com/yungSNAT/Alcuin/main/install.sh | sh
          echo "$HOME/.local/bin" >> "$GITHUB_PATH"
      - name: Review the changed files
        env:
          OPENAI_API_KEY: ${{ secrets.OPENAI_API_KEY }}
          BASE: ${{ github.event.pull_request.base.sha }}
        run: |
          git diff --name-only --diff-filter=d "$BASE"...HEAD > changed.txt
          if [ ! -s changed.txt ]; then echo "No changed files to review."; exit 0; fi
          mapfile -t files < changed.txt
          alcuin --engine api --wait --write-output review.md \
            -p "Review this pull request's changes. List defects first, each with its file and line." \
            --file "${files[@]}"
      - name: Post the review
        if: ${{ hashFiles('review.md') != '' }}
        env:
          GH_TOKEN: ${{ github.token }}
          PR: ${{ github.event.pull_request.number }}
        run: |
          gh pr comment "$PR" --body-file review.md
```

What it does and why:

- The `git diff` line lists the files the pull request changed since it left its base, leaving out deleted ones ([Files and limits](files.md#choose-files)).
- Each review is billed for its files and answer. From v0.2.0, add `--max-cost <dollars>` to refuse a run whose worst-case cost is over your cap before anything is sent ([Configuration](configuration.md#a-spend-cap)); v0.1.0 refuses the option.
- Alcuin's checks still apply: a file over the size limit, or one that looks like a credential, stops the run and nothing is sent.
- The job reads the pull request's files but never runs its code, and the checkout keeps no token. Only the comment step is handed the job's token, which can write to pull requests and nothing else.
- To review only when someone adds the label `alcuin-review`, give `pull_request:` the line `types: [labeled]` and add `&& github.event.label.name == 'alcuin-review'` to the `if:`.

The install step runs `install.sh` from `main`, which installs the latest release after checking its archive's SHA-256. To pin a release, pipe it to `ALCUIN_VERSION=v0.1.0 sh` instead of `sh`.
