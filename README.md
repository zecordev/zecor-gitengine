# zecor-gitengine

In-process git: branch-state, merge-base, rev-parse, tree diff -- no `git` subprocess.

Part of [Zecor](https://zecor.dev) -- an autonomous software construction engine.
Apache-2.0. Prebuilt binaries for Linux / macOS / Windows are attached to each
[release](https://github.com/zecordev/zecor-gitengine/releases); or `cargo install zecor-gitengine`.

## 5. `zecor-gitengine` — in-process git

**Incumbents.** `libgit2`/`git2-rs` (C, mature, but a C dep and not fully thread-safe),
`gitoxide`/`gix` (pure Rust, fast, thread-safe, the clear future), shelling out to `git`
(what every agent does — thousands of forks a night, each ~5-15ms of process overhead).

**Gaps.** Agents call `git status`, `git diff`, `git rev-parse`, `git merge-base`,
`git apply`, `git worktree` constantly. Each is a fork+exec+parse. `gix` makes all of
these in-process µs-scale calls, but nobody has packaged the *specific set an agent
needs* behind a stable JSON CLI.

**Shipped.** `head`, `rev-parse`, `merge-base`, `changed-files` (tree/tree), and
`branch-state` (ahead/behind, is-merged, is-descendant, merge-base as one call -- the
auto-merge gate's check). **`log [rev] --limit N`** -- sha, summary, author, epoch time,
parents, newest first. **`status`** -- staged (HEAD vs index, by blob id), unstaged
(index vs worktree via `gix` status), untracked; `is_clean`. All read-only. Mirrored in
`zecor.gitengine` with a `git` fallback; exposed as `zecor git {log,status,...}`.

**Still to world-class.**
- **`diff <a> <b>`** — unified text + a structured hunk list; binary-aware; index/worktree.
- **`apply <patch>`** — three-way patch application with conflict reporting, no
  worktree mutation unless `--write`.
- **`blame <file> --range L1,L2`** — "which symbol/commit does this line trace to".
- **`worktree add/remove/list`** — lane setup without a subshell.
- **Rename detection** in `status` and `changed-files`.
- **`commit-graph` cache** — warm `gix` object DB, shared read handle across lanes.

## Build

```
cargo build --release      # -> target/release/zecor-gitengine
cargo test --all-targets
```
