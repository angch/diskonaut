# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `--bench-stage tree-only` times the folder tree with the walk taken out of the measurement, so
  the model's cost can be read directly instead of inferred from `walk` against `tree`.

### Changed

- Linux scans use a native walker (`getdents64` + `statx`) with its own worker pool instead of
  `dua-core`. A whole-volume scan of a 4.2M-entry XFS filesystem went from 2.9s to 0.7s in the
  app's load path, and the traversal alone from 2.3s to 0.5s. `dua-core`'s walk stopped scaling
  well before the kernel did — sixteen independent walker processes over the same tree reached
  13.3M entries/s while its own workers peaked at six. The scan worker cap on Linux rises from 8
  to 24 accordingly. See `docs/scan-performance.md`.

- Tree building on Linux is about twice as fast and uses half the memory: hard-link accounting
  works over interned directory ids instead of re-parsing paths, folders are boxed inside
  `FileOrFolder` so a file's map slot no longer pays for a folder, and the folder and inode maps use
  a fast non-cryptographic hasher. On a 2.2M-entry ext4 volume the app's load path went from ~5.2s
  to ~2.4s, and peak RSS from ~870 MB to ~430 MB. See `docs/scan-performance.md`.

### Known issues

- The scan is now bound by tree building rather than traversal: the walk is 0.43s of a 0.72s scan
  on a 4.2M-entry volume and is fully hidden behind the model. Filesystem-specific metadata APIs
  would buy about 3% even if they were free and available, which they are not.

- A scan of `/` double-counts any filesystem mounted in more than one place — on a machine with
  one filesystem at both `/data` and `/home` it reports 1.8 TiB against about 1 TiB held. The
  double count is not new, but `/` now finishes fast enough for anyone to see it. Use `-x` for a
  trustworthy whole-machine total.

### Fixed

- Directory recursion is filesystem-aware: `/proc`, `/sys`, cgroup, debugfs and the other
  pseudo-filesystems are no longer descended into when a scan crosses a mount point into one. They
  report no disk usage, `/proc` grows while the scan runs, and parts of it fail permanently when the
  process they describe exits. Scanning one by name still works — `diskonaut /proc` walks `/proc`.
  A scan of `/` went from not finishing in ten minutes to 2.0s. `tmpfs` is still counted, as `du`
  counts it.
- A directory whose `getdents64` failed part-way was read again forever rather than abandoned,
  which hung the scan. `/proc/<pid>/net` for an exited process returns `EINVAL` on every call and
  triggered it reliably. A signal arriving mid-read (`SIGWINCH`, i.e. resizing the terminal while
  a scan runs) is retried rather than treated as a dead directory, which would have dropped the
  rest of that directory and its subtree.
- The treemap no longer panics on a folder whose entries are bigger than the folder holding them.
  Shared blocks — hard links, and now reflinks — make a folder smaller than the sum of its
  contents, so entry shares could add up to more than the whole board (four reflinked copies of one
  file gave 4.0) and tiles were laid out off the screen, where the renderer indexes the terminal
  buffer directly and crashed. Shares are now taken against the larger of the folder and its
  contents, and a tile that does not fit is skipped rather than drawn.
- A file sharing only part of itself with another is counted in full rather than merged with it.
  Identity is the whole extent map, not the first extent, which could otherwise halve a total for
  two equal-sized files that shared nothing but their opening extent.
- Scanning no longer triggers automounts. The walk's `statx` lacked `AT_NO_AUTOMOUNT`, which
  `stat`/`lstat`/`fstatat` imply but `statx` does not, so merely stating an autofs placeholder
  mounted it — a directory of NFS home maps would have been mounted wholesale.
- Sizes no longer count copy-on-write shared data twice. On XFS and btrfs a reflinked copy reports
  its full block usage with a link count of 1, so tools that de-duplicate on inode — including
  `du` — charge every copy in full. A scan of a `uv` package cache reported 15.1 GiB where 12.0 GiB
  is held, and a whole-volume scan overstated by 21.9 GiB. Shared extents are now charged to a
  folder once, the same rule hard links already followed. Files under 64 KiB, and filesystems that
  cannot share extents, are not probed.
- The treemap silently dropped every entry too small to draw when their combined tile rounded to
  zero cells, which happens whenever one entry (a `target/` or `.git/`, say) holds nearly all of a
  folder. The board then showed that single entry at 100% with no "small files" marker and no
  legend, as if the scan had missed the rest. The `x` marker is now always drawn for hidden
  entries, clamped to at least a few cells inside the board, and zoom (`+`) reveals them as before.
- The help line at the bottom of the screen still advertised `<BACKSPACE> - delete`, a key that
  has done nothing since keybinds became configurable with `d` as the default. The help line is now
  built from the configured keybinds, so it always names the keys that actually work. To keep
  Backspace, set `delete = "backspace"` in `~/.config/diskonaut/config.toml`.

## [0.13.0] - 2026-09-04

### Changed

- Directory walk: **`jwalk` → `dua-core`**.
- `ratatui`'s `crossterm` feature is used instead of a direct `crossterm` dependency.
- `README.md` trimmed down.
- Dependency bumps: `clap` 4.6.5 → 4.6.6, `thiserror` 2.0.18 → 2.0.20, `toml` 1.1.2+spec-1.1.0 → 1.1.4+spec-1.1.0, `ratatui` 0.30.0 → 0.30.2, `actions/checkout` 6 → 7, `codecov/codecov-action` 6 → 7.

### Fixed

- TUI not rendering on initial start, caused by a race between the stdin event-reader thread and the terminal's cursor-position query during startup.

### Added

- `docs/ARCHITECTURE.md`.

## [0.12.2] - 2026-05-28

### Added

- TOML config (`version = 1`, `[base]`, `[keybinds]`) with default `~/.config/diskonaut/config.toml` and `-c` / `--config` override; see `example/config.toml`.
- `libdiskonaut` uses the repository root `README.md` on [crates.io](https://crates.io/crates/libdiskonaut).

### Changed

- Delete keybind: `Backspace` → `d`.

### Removed

- `-x` / `--disable-delete-confirmation` CLI flag; deletions always require confirmation.

## [0.12.1] - 2026-05-28

### Changed

- Fixed `Deploy` job

## [0.12.0] - 2026-05-28

### Added

- Cargo workspace with **`libdiskonaut`** (scan, model, treemap, formatting) and **`diskonaut`** (CLI + TUI).
- Unit tests colocated per module (`tests.rs` siblings) in both crates.
- GitHub Actions CI: `fmt`, `typos`, `cargo deny`, `clippy`, `test`, and `doc` workflows.
- Block-usage sizing on Unix via `rustix` / `st_blocks` (replaces the `filesize` crate).
- CLI flags unchanged in spirit: `-a` / `--apparent-size`, `-x` / `--disable-delete-confirmation`, optional scan path argument.

### Changed

- Rust **2024** edition (workspace).
- TUI stack: **`tui` → `ratatui`** (with `crossterm` 0.29).
- CLI: **`structopt` → `clap` v4** (derive).
- Errors: **`failure` → `thiserror`** at crate boundaries.
- POSIX helpers: **`nix`, `filesize` → `rustix`** (e.g. admin / root indicator).
- Directory walk: **`jwalk` 0.8**.

### Removed

- **Windows** support (`winapi`, Windows-specific OS code, and Windows CI).
- **`insta`** snapshot / integration UI tests (replaced by focused unit tests; manual TUI smoke test for UI).
- Dependencies dropped as part of the migration: `failure`, `structopt`, `nix`, `filesize`, `tui`.
