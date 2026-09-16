---
name: land-pr
description: "Lands a tinybrowser change from branch to merged main: adversarial review, draft pull request, CodeRabbit triage, merge, and sync. Use when the user asks to land, ship, or open a PR for a change in this repository. Skip for analysis-only work and throwaway spikes."
---

# Land a PR

One change, one branch, one draft PR, one merge. Follow the steps in order.

## Rules that shape the flow

- Read `AGENTS.md` first. It holds the hard rules: no handwritten unsafe, no
  lint escapes, spec citations, and no cargo tests for web-platform behavior.
- Work in a worktree, not the primary checkout. `main` lives in the primary
  checkout (`/home/erickc/projects/tinybrowser`), and a branch can only be
  checked out in one worktree. If the primary sits on a feature branch, do not
  move it without asking.
- Run cargo inside the devshell: `nix develop --command cargo …`.
- Keep one PR. Do not stack PRs unless the user asks.
- The repository has no GitHub Actions. CodeRabbit and local runs are the
  only gates.

## 1. Branch

```sh
git switch -c <type>/<slug>
```

Use `feat/`, `fix/`, `chore/`, `refactor/`, or `spike/`.

Commit one story per commit, in the shape `type(scope): imperative subject`.
Put the reasoning in the body: why this design, what it rejected, what the
verification showed. Decisions live in commit messages, not in docs.

## 2. Verify locally

```sh
nix develop --command cargo clippy --workspace --all-targets -- -D warnings
nix develop --command cargo test --workspace
nix develop --command cargo build --release --bin tinybrowser
stat --format='%n %s' target/release/tinybrowser
git diff --check
```

The binary must stay under 10MB stripped. Report the size when it changes.

Web-platform changes also run WPT:

```sh
nix develop --command ./tools/wpt/run <test paths...>
```

Any worktree can run WPT. A worktree without its own `third_party/wpt` uses
the primary checkout's copy, and the script checks that the pinned revision
matches. `TINYBROWSER_BINARY=…` tests a prebuilt binary instead of building
one. One run at a time: the script locks the shared venv.

When the harness cannot cover a change, say so in the PR body under
`## Known verification gap`, with the reason and what would close it. PR #12
is the model.

## 3. Draft PR

```sh
git push -u origin <branch>
gh pr create --draft --base main --title "<title>" --body "<body>"
```

The body carries the summary, the verification commands with their results,
and any known gap. Mark it ready with `gh pr ready` only when the user
confirms or the review is done.

## 4. Adversarial review

Spawn two to four subagents with fresh context and one focus each, for
example spec conformance, memory and borrow safety, or scheduling and
lifecycle. Tell each one:

- Read the `review` and `improve-codebase` skills first, and report only;
  do not edit files.
- Review the exact range `git diff <base>..HEAD` in the given worktree.
- Verify against the governing spec and cite it, or read a shipped browser.
- Label every finding with Severity (must-fix / should-fix / nit) and Kind
  (bug / unverified / security / test / comment / design / clarity), with
  `file:line` and a concrete failing scenario.
- State what was verified correct and how.
- Use the evidence already collected instead of re-running heavy builds.
- Keep the report under about 1200 words and skip generic advice.

Consolidate the reports, dedupe, and fix what holds up. Push the fixes.

## 5. CodeRabbit

After each push, run the watcher in the background:

```sh
tools/dev/coderabbit-wait <pr>
```

It notifies when the pass for the current head ends. Read the review yourself:

```sh
REPO=ericc-ch/tinybrowser
gh api "repos/$REPO/pulls/<pr>/comments" \
  -q '.[] | "\(.path):\(.line // "general") [\(.commit_id[0:7])]\n\(.body)"'
gh pr view <pr> --json reviews,statusCheckRollup
```

Triage rules:

- Dedupe findings that repeat across threads.
- Fix only high-signal findings. Verify each one against the spec or the code
  before applying it; one CodeRabbit fix broke an Intl test.
- Skip the docstring-coverage threshold. It is a bot metric, not a defect.
- Defer out-of-scope work in a PR comment with a reason, not silently.
- Reply to every addressed thread with `Addressed in <sha>: <what changed>`.
- Wait for the re-review of the new head before merging.

## 6. Merge

Use a merge commit. Do not squash or rebase.

```sh
gh pr merge <pr> --merge --delete-branch
```

Run it from the primary checkout when it sits on `main`. From a worktree,
merge without `--delete-branch`, then detach and delete the branch after the
sync. Keep long-lived branches such as `refactor/v2-async-browser` and
`spike/*`.

## 7. Sync

```sh
git -C /home/erickc/projects/tinybrowser checkout main
git -C /home/erickc/projects/tinybrowser pull --ff-only origin main
```

Then reset the session worktree:

```sh
git fetch --prune origin
git switch --detach origin/main
git branch -D <branch>    # once the remote branch is gone
```

## Disk policy

Every worktree builds its own `target/`, and a complete one is many
gigabytes. Keep one build-heavy worktree at a time. After the merge:

```sh
nix develop --command cargo clean
git -C /home/erickc/projects/tinybrowser worktree remove <path>
```

Check `df -h` and `du -sh` before starting a fresh build. Do not add
`sccache` or a shared `CARGO_TARGET_DIR` without measuring first.

## Signing

Commits sign through the SSH key and `~/.local/bin/ssh-sign-with-agent`. If
the agent is locked, signing fails; use `git -c commit.gpgsign=false commit`
for local commits and tell the user. Check a commit with
`git log -1 --format='%G?'`; `G` means a good signature.
