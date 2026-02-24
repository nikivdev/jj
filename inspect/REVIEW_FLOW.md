# Flow commit + JJ review stack (inspect integration)

This doc extends `/Users/nikiv/code/nikiv/docs/flow/f-commit.md` with the **stacked review** and **inspect TUI** workflow.
It is meant to be fed to AI tools so they can suggest the correct review/approve flow.

## Goal
- **Never push on `f commit`** (queue by default).
- **Review queued commits as a stack** (base → top).
- **Approve only after review** (explicit action).
- **Use JJ for stack math**, but remain Git‑compatible.

## Key Concepts
- **Commit queue**: `f commit` writes queue entries to `.ai/internal/commit-queue/*.json` and creates a JJ review bookmark (e.g. `review/main-<sha>`).
- **Stack**: a linear sequence of commits between a base (e.g. `main`) and the working copy; JJ makes these easy to list.
- **Inspect TUI**: `jj-inspect` shows the queued commits (or a JJ stack), lets you browse files/diffs, run commands, and approve.

## Primary Flow (repo like `~/code/rise`)

### 1) Make changes
Edit files as usual.

### 2) Queue a commit (no push)
```bash
f commit
```
Expected:
- Commit is created locally.
- A queue entry is written.
- A review bookmark like `review/main-<sha>` is created.
- **No push** happens unless you explicitly approve.

### 3) Review the queued commits (TUI)
```bash
inspect --queue --repo ~/code/rise
```
UI behavior:
- **Left**: file list (status + path)
- **Right**: diff preview for selected file
- **Bottom**: hints + command prompt

Keys:
- `[` / `]` → previous/next commit in queue
- `j` / `k` → move file selection
- `Enter` → open full diff (pager)
- `:` → command mode (runs shell in repo)
- `A` → approve selected commit (queue mode)
- `q` → quit

### 4) Approve (push)
From the TUI, press `A` to run:
```bash
f commit-queue approve <sha>
```
This is the **only** action that pushes.

## Stack Review (JJ mode)
If you want to see the *stack* of local changes instead of just the queue:
```bash
inspect --repo ~/code/rise --base main
```
This shows `ancestors(@) & ~ancestors(main)` as a stack.

## Alternative Review Entry Points
- Open in Rise app:
  ```bash
  rise review open --queue <sha>
  ```
- List queued commits:
  ```bash
  f commit-queue list
  ```
- Show queued commit:
  ```bash
  f commit-queue show <sha>
  ```

## Guardrails (must‑follow)
- **Do not push from `f commit`.** Queue is default.
- **Do not `f sync` with a non‑empty queue** unless you pass `--allow-queue` (rebases rewrite SHAs).
- **Review order matters**: base → top, because later commits can depend on earlier ones.

## Common Fixes
- If `.beads` files appear in the diff, remove them and ensure `.beads/` is in `.gitignore`.
- If queue entries drift after rebase, re‑list the queue or re‑queue commits.
- If `inspect` isn’t found:
  ```bash
  f inspect-deploy
  ```

## Example (end‑to‑end)
```bash
cd ~/code/rise
f commit                      # queue only
inspect --queue --repo .      # review + approve
```

## Why this structure
- Keeps Git compatibility (queue is local, pushes are explicit).
- JJ makes stack ordering reliable and easy to inspect.
- Inspect gives a fast review loop without leaving the terminal.
