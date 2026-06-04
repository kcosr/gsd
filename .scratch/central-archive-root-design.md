# Central Archive Root Design Note

## Recommendation

Supporting a configured archive root is feasible and probably useful, but it
should be introduced as an explicit storage mode rather than quietly replacing
the colocated `.gsd/` default. The current implementation already uses
`git --git-dir --work-tree`, which is the right primitive, but many APIs derive
`target/.gsd` internally. The safe implementation is a small storage resolver
that returns `{ target_path, git_dir }` for every git operation and CLI check.

Use colocated `.gsd/` as the default. Add an optional global archive root under
`[git]`, for example:

```toml
[git]
archive_root = "/var/lib/gsd/archives"
```

When configured, every target gets a bare git directory under that root. Avoid
automatic migration in the first implementation.

## Current `.gsd` Assumptions

- `src/git/mod.rs` defines `GSD_DIR = ".gsd"` and `run_snapshot_git()` always
  passes `--git-dir=.gsd --work-tree=.` with `current_dir(target)`.
- `ensure_repo_initialized()`, `check_repo_ownership()`,
  `sync_snapshot_excludes()`, config writes, and tests all derive
  `target/.gsd` internally from only `dir: &Path`.
- `src/main.rs` directly checks or deletes `target/.gsd` in `snapshot`, `git`,
  `remove`, `check`, preview ignore reporting, and `is_always_excluded()`.
- `src/snapshot/mod.rs` stores target state by target path and calls git helpers
  with only the target path, so the daemon cannot currently distinguish storage
  location from work tree.
- `README.md` documents `.gsd/` as the snapshot repository and `.gsd/info/exclude`
  as generated state.

## Git Invocation Feasibility

The project is already close to the required git model. For CLI `gsd git`,
`main.rs` invokes:

```text
git --git-dir <target/.gsd> --work-tree <target> ...
```

The async helper currently invokes the same model with relative paths:

```text
git --git-dir=.gsd --work-tree=. ...
```

Central storage mainly requires changing snapshot helpers to accept an explicit
git directory:

```rust
struct SnapshotRepo {
    work_tree: PathBuf,
    git_dir: PathBuf,
}
```

Then all commands should use absolute paths:

```text
git --git-dir <archive-root>/<archive-id>.git --work-tree <target> ...
```

This also avoids subtle current-directory dependence.

## Naming and Collision Strategy

Do not use raw target names alone. They collide (`/a/foo` and `/b/foo`) and can
contain awkward path characters.

PI comparison: `/home/kevin/worktrees/pi` uses a readable cwd encoding for
session storage:

```text
--<absolute-path-with-leading-slash-removed-and-/,\,:-replaced-by-->--
```

For example, `/tmp/my-project` becomes `--tmp-my-project--`. This is much more
readable than using only a leaf name, but by itself it is not collision-safe:
`/a-b`, `/a/b`, and `/a:b` all collapse to similar names. It also has no length
guard for deeply nested paths.

Recommended archive name:

```text
<sanitized-full-path>.<stable-hash>.git
```

Where:

- `sanitized-full-path` is a PI-style readable encoding of the canonical
  absolute target path, using the whole path rather than only the leaf.
- `stable-hash` is computed from the canonical absolute target path.
- The stored repository can use standard `core.worktree` metadata for manual
  git inspection. GSD daemon and CLI operations should still pass explicit
  `--git-dir` and `--work-tree` values instead of depending on repository-local
  metadata.

This keeps archive directories inspectable without relying on the readable name
for identity. The hash should remain mandatory because readable path
sanitization is lossy. Add a real digest crate rather than using Rust's standard
hashing, which is not appropriate for long-term path identity.

Use a maximum filename budget. If `<sanitized-full-path>.<hash>.git` would be
too long, truncate the readable prefix and keep the full hash suffix. The full
canonical path remains recoverable from repository metadata, not necessarily
from the directory name.

## Migration and Backward Compatibility

Do not auto-migrate `target/.gsd` into the archive root in the first release.
Automatic migration has too many failure modes: ownership differences,
cross-filesystem moves, interrupted moves, and surprising history relocation.

Implemented smallest safe behavior:

- No `archive_root`: continue using `target/.gsd`.
- `archive_root` configured and central archive exists: use it.
- `archive_root` configured and central archive does not exist: create a new
  central archive. Existing `target/.gsd` history is not migrated or reused.
- `archive_root` configured and `target/.gsd` exists: leave it untouched and
  exclude `.gsd/` from central snapshots so old colocated history is not
  ingested as target content.
- Future migration command can copy/move with validation, preserve permissions,
  preserve usable work-tree metadata, and leave source `.gsd` untouched unless the user
  confirms deletion.

Given the project guidance to avoid dual-shape parsers and compatibility layers,
prefer an explicit storage mode decision over opportunistically supporting both
locations for a single target.

## Security and Permissions

Central storage can improve targets like `/etc` because gsd no longer needs to
write `.gsd/` or append `.gsd/` to `/etc/.gitignore`. It still needs read access
to snapshot files and may need access to `.gitignore` / `.gsdignore`.

Main risks:

- If gsd runs as root and archives are under a user-writable root, a malicious
  user could replace archive directories or paths. The archive root must be
  owned by the gsd user/root and not group/world writable.
- If one daemon monitors mixed-ownership targets, central archives may expose
  private content from multiple owners in one directory. Use restrictive
  permissions (`0700`) by default.
- Git honors repository config and hooks in the git directory. Disable or avoid
  hooks for daemon operations, and never allow an archive path controlled by a
  less-privileged target owner when running privileged.
- Symlink and canonicalization behavior must be explicit. Hash and store the
  canonical target path, and validate that the resolved target is still the
  configured target before committing.
- For system paths, writing `.gsdignore` may not be possible. Config-level
  ignore patterns should remain sufficient.

## Ignore Behavior

Target-local `.gitignore` and `.gsdignore` can continue to work. The generated
exclude file should move from `target/.gsd/info/exclude` to
`<resolved-git-dir>/info/exclude`.

Preview needs the same storage resolver so it can report/read the central
`info/exclude`. The generated exclude file should always reserve `.gsd/`,
including central mode, so old target-local snapshot archives are not captured
after enabling `archive_root`. `is_always_excluded()` should keep excluding
`.git`; central mode can rely on the generated `info/exclude` for `.gsd/`.

Important policy decision: when central storage is configured, `gsd add` should
not append `.gsd/` to the target `.gitignore`, because no colocated archive is
created. That is especially important under `/etc` or read-only targets.

## Pros and Cons

Pros:

- Works better for targets where writing hidden state inside the target is
  undesirable or impossible, especially `/etc`.
- Keeps monitored directories cleaner and avoids touching their `.gitignore`.
- Makes backup/retention of snapshot archives easier because all histories live
  under one root.
- Can use tighter permissions on archive history than the target directory
  itself.

Cons:

- Less obvious to users where history lives.
- Requires new archive naming, identity, and collision validation.
- Makes `remove`, `check`, `preview`, and `gsd git` depend on config to resolve
  the archive location.
- Mixed-ownership targets become easier to misconfigure in unsafe ways.
- Migration from existing `.gsd` directories needs an explicit workflow.

## Smallest Safe Implementation

1. Add `git.archive_root: Option<PathBuf>` to config and validate it is absolute
   when present.
2. Introduce a `SnapshotRepo`/`SnapshotStorage` resolver:
   - input: config git settings + target path
   - output: absolute `work_tree` and `git_dir`
   - default: `target/.gsd`
   - central: `<archive_root>/<sanitized-full-canonical-path>.<hash>.git`
3. Change git helpers to accept `&SnapshotRepo` instead of only `&Path`.
4. Make all git invocations pass absolute `--git-dir` and `--work-tree`.
5. Move generated exclude sync to `repo.git_dir/info/exclude`, while still
   reading `.gitignore` and `.gsdignore` from the target work tree.
6. Update CLI commands to load config where needed before resolving storage:
   `snapshot`, `git`, `remove`, `check`, and `preview`.
7. Make `add` create the archive root with `0700` permissions on Unix when
   central storage is configured; do not modify target `.gitignore` just to add
   `.gsd/` in central mode.
8. Add tests for colocated mode, central mode, collision-resistant naming,
   preview ignores, `gsd git`, daemon reload, and behavior when an existing
   colocated `.gsd` is present under central storage.

Advisable first release behavior: central storage is opt-in, non-migrating, and
explicit. Document that existing `.gsd` targets should remain colocated until a
separate migration command is added.
