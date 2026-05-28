# diskonaut — Rust architecture + implementation plan

Human roadmap and agent playbook for modernizing **diskonaut**: a terminal disk-space treemap navigator.

Execution discipline mirrors [`docs/TRAYD_PLAN.md`](TRAYD_PLAN.md) and the `trayd/` reference tree in this repo:

- **Library-first** crate split, small verifiable phases, strict quality gates.
- **Directory modules** with sibling **`tests.rs`** — production logic and tests are never mixed in the same file.
- **Unix-only** after this effort (Windows support removed).

**Design authority:** this document + the numbered goals in the migration issue/PR. On conflict, newest revision of `PLAN.md` wins.

---

## 1. Goals (summary)

| # | Goal | Target |
|---|------|--------|
| 1 | Workspace layout | Root `Cargo.toml` workspace: **`libdiskonaut`** + **`diskonaut`** binary crate |
| 2 | Testing | `libdiskonaut/` + `diskonaut/` unit tests via `tests.rs` siblings; no integration/snapshot tests |
| 3 | CI/CD | GitHub Actions: fmt, typos, deny, clippy (feature matrix), test matrix, doc |
| 4 | Quality gates | All jobs green on every merged phase |
| 5 | Edition | Rust **2024** (workspace) |
| 6 | TUI | **`tui` → `ratatui`** (+ current `crossterm`) |
| 7 | POSIX | **`nix` → `rustix`** (`geteuid` / admin check) |
| 8 | Errors | **`failure` → `thiserror`** (+ `std::error::Error` at binary edge) |
| 9 | Walk | **`jwalk` → modern parallel walker** (see §4.3; not the `jw` CLI crate) |
| 10 | CLI | **`structopt` → `clap` v4** (derive) |
| 11 | Block usage | **Drop `filesize`**; inline `st_blocks` via `rustix`/`libc` on Unix |
| 12 | Platforms | **Remove Windows** (`winapi`, `os/windows.rs`, cfg branches) |
| 13 | Deps | Bump all workspace dependencies; `cargo deny` + advisories |
| 14 | Snapshots | **Removed** — integration/UI tests dropped; `libdiskonaut` unit tests only |

---

## 2. Repository layout (target)

```text
diskonaut/                       # workspace root (this repo)
  Cargo.toml                     # members: libdiskonaut, diskonaut
  deny.toml
  .typos.toml
  libdiskonaut/
    Cargo.toml
    src/
      error.rs
      scan/                      # WalkDir / filesystem traversal
      model/                     # FileOrFolder, Folder, File, FileTree
      tiles/                     # treemap, board, rect layout
      format/                    # display_size, truncate (non-TUI)
      os/                        # unix: block size, is_root (rustix)
  diskonaut/
    Cargo.toml
    src/
      main.rs                    # clap, tokio/thread spawn, run loop
      app/                       # App state machine, UiMode
      input/
      messages/
      ui/                        # ratatui widgets, modals, grid
      cli/ input/ app/ ui/ …   # each with tests.rs sibling
  docs/
    PLAN.md
  .github/workflows/
```

### 2.1 Crate boundary rules

| Crate | Responsibility | Allowed deps | Forbidden |
|-------|----------------|--------------|-----------|
| **libdiskonaut** | Scan, tree model, treemap math, size formatting helpers | `jwalk` (or successor), `rustix`, `thiserror`, `unicode-width` | `ratatui`, `crossterm`, `clap` |
| **diskonaut** | TUI, input loop, delete UX, CLI | `libdiskonaut`, `ratatui`, `crossterm`, `clap`, `thiserror` | Reimplementing treemap/scan in the binary |

---

## 3. Quality gates (must pass each phase)

Run from workspace root:

```bash
cargo fmt --all -- --check
typos
cargo deny check
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --no-default-features
cargo test --workspace --all-features
cargo doc --workspace --no-deps --all-features
```

**CI:** Linux only (`ubuntu-latest`). No Windows matrix.

**Coverage (optional):** `cargo llvm-cov` per crate, Codecov flags `libdiskonaut` and `diskonaut` (fix workflows that still reference trayd crate names).

---

## 4. Dependency migrations

### 4.1 `failure` → `thiserror`

- `libdiskonaut::DiskonautError` for scan/model errors.
- `diskonaut::Error` for terminal/UI errors; `main` prints `Display` and exits `2`.
- Remove `failure` and `failure_derive` entirely.

### 4.2 `structopt` → `clap`

Preserve flags:

- positional `folder: Option<PathBuf>`
- `-a` / `--apparent-size`
- `-x` / `--disable-delete-confirmation` (keep short flag semantics or document breaking change)

### 4.3 `jwalk` → walker policy

**Do not** depend on the crates.io **`jw`** package — it is a **CLI frontend for jwalk**, not a library API.

**Recommended:** upgrade **`jwalk` to 0.8.x** and adapt `Parallelism` / `WalkDir` API changes in `libdiskonaut::scan`.

**Alternative (if API fit is poor):** `dirwalk` or `walkdir` + `rayon` — justify in PR.

### 4.4 `filesize` removal

`filesize::PathExt::size_on_disk_fast` is used only for **allocated blocks** when `--apparent-size` is off.

Replace with a small Unix helper in `libdiskonaut::os`:

```rust
// st_blocks * ST_BLKSIZE (typically 512 on Linux; use fstat on fd when possible)
```

Tests use `--apparent-size` / `SHOW_APPARENT_SIZE` so CI stays filesystem-agnostic.

### 4.5 `nix` → `rustix`

- `geteuid().is_root()` for admin indicator in title bar.
- Drop `nix` crate.

### 4.6 `tui` → `ratatui`

- Workspace dep: `ratatui` with `crossterm` feature (match trayd/tray-tui style).
- Update imports: `tui::` → `ratatui::`.
- Revise `TestBackend` in tests to implement `ratatui::backend::Backend` (buffer type may differ).
- Revisit crossterm version (align with ratatui MSRV).

---

## 5. Windows removal

Delete:

- `Cargo.toml` `[target.'cfg(windows)'.dependencies]`
- `src/os/windows.rs` and `mod windows`
- `#[cfg(target_os = "windows")]` in `title_line.rs` and tests
- Appveyor / Windows docs in README (when touched)

`libdiskonaut::os` is `unix` only; compile on Linux/macOS/BSD.

---

## 6. Testing

**libdiskonaut:** unit tests for treemap layout, folder aggregation, delete_path, format helpers — pure data, no terminal (`mod tests;` siblings per module).

**diskonaut:** same `tests.rs` sibling pattern per module (`cli`, `error`, `input`, `app`, `messages`, `state`, `ui/grid`, `ui/title`, …). No integration/UI snapshot tests; TUI smoke-tested manually.

---

## 7. Phased steps

### Phase 0 — Workspace scaffold + CI fix

- [x] Root workspace `Cargo.toml` (`resolver = "3"`, edition `2024`).
- [x] Create `libdiskonaut/` and `diskonaut/` crates; move `src/` → split per §2.
- [x] Fix `.github/workflows/*` crate names (`libdiskonaut`, `diskonaut` — not trayd).
- [x] `deny.toml` licenses/advisories populated (MIT).
- [x] `cargo fmt`, `clippy`, `test` green on moved code **without** dependency migrations yet (minimal diff).

**Verify:** §3 gates on scaffold.

### Phase 1 — `libdiskonaut` extraction

- [x] `scan/`, `model/`, `tiles/`, `format/`, `os/` with `tests.rs` siblings.
- [x] Public API: `scan_folder`, `FileTree`, treemap types used by the binary.
- [x] No `ratatui` in lib.

**Verify:** lib unit tests; binary still runs (thin wrapper).

### Phase 2 — Error + CLI + POSIX deps

- [x] `thiserror`, `clap`, `rustix`; remove `failure`, `structopt`, `nix`.
- [x] Remove Windows code.

**Verify:** §3 gates; CLI `--help` unchanged in spirit.

### Phase 3 — Walker + block size

- [x] `jwalk` 0.8 (or chosen walker).
- [x] Inline block-size; remove `filesize`.

**Verify:** scan tests + manual run on large directory.

### Phase 4 — `ratatui` migration

- [x] All UI under `diskonaut/src/ui/` on ratatui.
- [x] Update `TestBackend` for ratatui.

**Verify:** manual TUI smoke test.

### Phase 5 — Remove integration tests

- [x] Delete `diskonaut/src/tests/` (UI tests, fakes, insta snapshots).
- [x] Remove `insta` dev-dependency.

**Verify:** `cargo test --workspace` (libdiskonaut unit tests only).

### Phase 6 — Dependency refresh + docs

- [ ] `cargo update` / workspace dependency policy.
- [ ] README, CHANGELOG, MSRV note.
- [ ] Tag release when ready.

---

## 8. Definition of done (v0.12 or next minor)

- [ ] Workspace `libdiskonaut` + `diskonaut` with tests colocated per module.
- [ ] Edition 2024; ratatui; clap; rustix; no failure/nix/structopt/filesize/insta/windows (integration tests removed).
- [ ] CI green on all §3 gates.
- [ ] Linux/macOS supported; Windows explicitly out of scope.

---

## 9. Reference material in this repo

- **`trayd/`** — nested copy for workspace/CI/test layout patterns (not a workspace member).
- **`docs/TRAYD_PLAN.md`** — quality-gate and module/test conventions.

---

## Revision history

| Date | Change |
|------|--------|
| 2026-05-28 | Initial diskonaut migration plan (workspace, deps, testing, CI) |
