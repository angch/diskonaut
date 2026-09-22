# AGENTS.md — Diskonaut Agentic Development Guide

## Project Overview

**Diskonaut** is an interactive terminal disk space navigator (TUI) written in Rust. It visualizes
disk usage via a squarify treemap, supports live scanning, and allows deleting large files in-place.

**Workspace layout** (Rust 2024 edition, version 0.13.0):
```
diskonaut/
├── libdiskonaut/     # Core library: model, scan, treemap, formatting, os
├── diskonaut/        # TUI binary: CLI, UI, app state, input, config
├── example/config.toml
└── Cargo.toml        # Workspace root
```

---

## Essential Commands

```bash
cargo build --workspace          # Build everything
cargo test --workspace           # Run all tests
cargo clippy --workspace --all-targets -- -D warnings   # Lint
cargo fmt --all                  # Format code
cargo fmt --all -- --check       # Format check (CI)
```

**Run the binary:**
```bash
cargo run --bin diskonaut -- [FOLDER]
cargo run --bin diskonaut -- -a  # apparent size mode
```

---

## Architecture

### Thread Model

Five concurrent threads communicate via bounded `mpsc` channels:

| Thread | Role |
|--------|------|
| `stdin_handler` | Reads crossterm events → `Instruction::Keypress` |
| `hd_scanner` | parallel walk (its own worker pool) → `Instruction::AddScannedDirectories` (batched, ~4096 entries) |
| `event_executer` | Converts `Event` → `Instruction` (visual feedback) |
| `loading_loop` | Toggles loading indicator while scanning |
| **main** | App state mutations + ratatui rendering |

**Synchronization**: `Arc<AtomicBool>` for `running`/`loaded` flags; bounded sync channels (capacity 1–100).

### Crate Responsibilities

**`libdiskonaut`** — pure logic, no TUI:
- `model/files/file_tree.rs` — `FileTree`: hierarchical navigation, deletion tracking
- `scan/mod.rs` — `scan_directories()`: per-directory batches, the seam every walker plugs into
- `scan/bulk.rs` — macOS walker on `getattrlistbulk(2)` (see `docs/scan-performance.md`)
- `scan/linux.rs` — Linux walker on `getdents64`/`statx`, own thread pool; also the `FS_IOC_FIEMAP`
  reflink probe. `dua-core` is only the fallback for other platforms and the benchmark baseline
- `model/files/hard_links.rs` — charges shared blocks to each folder once, over interned directory
  ids; two ledgers, one keyed on inode (hard links) and one on physical extent (reflinks)
- `model/files/hash.rs` — the fast hasher behind the folder and inode maps
- `tiles/treemap.rs` — squarify algorithm (`HEIGHT_WIDTH_RATIO = 2.5`)
- `tiles/board.rs` — `Board`: tile selection, zoom stack, navigation
- `format/display_size.rs` — byte → human-readable (B/KB/MB/GB/TB)
- `os/unix.rs` — `is_user_admin()`, `size_on_disk_fast()`

**`diskonaut`** — TUI application:
- `main.rs` — entry point, thread spawning, channel setup
- `app/mod.rs` — `App` state machine, `UiMode` enum, render dispatch
- `input/controls.rs` — per-mode keypress handlers
- `messages/instruction.rs` — `Instruction` dispatch to `App` methods
- `ui/display.rs` — ratatui rendering orchestration
- `config/mod.rs` — TOML config (`~/.config/diskonaut/config.toml`)
- `cli/mod.rs` — clap CLI args

### UI State Machine (`UiMode`)

```rust
Loading                     // Scan in progress
Normal                      // Main treemap view
ScreenTooSmall              // Terminal < 50×15
DeleteFile(FileToDelete)    // Confirmation dialog
ErrorMessage(String)        // Error display
Exiting { app_loaded: bool }
```

---

## Key Patterns

- **Render-on-demand**: Render only when an `Instruction` arrives; no continuous loop.
- **Live treemap update**: `Board` recomputes tiles as scanned directories arrive.
- **Sizes are not additive**: a folder's size counts each distinct *set of blocks* once, so hard
  links and XFS/btrfs reflinks both make it smaller than the sum of its entries. See
  `docs/scan-performance.md`.
- **Zoom as filter**: Zoom level controls which nested folders are rendered.
- **Modal via enum**: `UiMode` variant change = modal open/close; no separate stack.
- **Config merging**: CLI `--apparent-size` ORs with config file setting.
- **Graceful degradation**: Read errors counted but scan continues. A failed *directory read* ends
  that directory rather than retrying it — the error belongs to the descriptor, not the entry.
- **Per-filesystem attributes**: a returned-attributes bitmap says an attribute is *present*, not
  that its value is real. `msdosfs` claims `ATTR_FILE_ALLOCSIZE` and packs zero, so the size
  attribute is chosen per device. See `docs/scan-performance.md`.
- **Pseudo-filesystems**: crossing a mount point into `/proc`, `/sys`, cgroup, debugfs and friends
  is refused (by `statfs` magic); naming one as the scan root still scans it.
- **ManuallyDrop on FileTree**: Avoids slow recursive drop on exit.

---

## Default Keybinds

| Action | Key |
|--------|-----|
| Quit | `q` |
| Delete | `d` |
| Navigate | `h/j/k/l` or arrow keys |
| Enter folder | `Enter` |
| Go to parent | `Esc` |
| Zoom in/out | `+` / `-` |
| Reset zoom | `0` |
| Confirm | `y` |
| Cancel | `n` |

---

## Code Conventions

- **Error handling**: `thiserror` derives; `?` propagation; distinct error enums per crate boundary.
- **Testing**: `#[cfg(test)] mod tests` in same file; temp dirs via helpers; setup → action → assert.
- **Concurrency**: Named threads; bounded channels; `park_timeout` (100ms) for polling.
- **Exports**: `pub use` re-exports in `mod.rs` files.
- **No async runtime**: Threads + channels only.
- **Unix-only**: No Windows support (removed in 0.12.0).

---

## Adding Features — Agent Guidance

### Adding a new keybind
1. Add field to `KeybindConfig` in `diskonaut/src/config/mod.rs`
2. Add parsing in `diskonaut/src/config/keybind.rs`
3. Add to `Keybinds` struct and wire in `input/controls.rs`
4. Update `example/config.toml`

### Adding a new UI mode
1. Add variant to `UiMode` in `app/mod.rs`
2. Add `handle_keypress_<mode>()` in `input/controls.rs`
3. Add render arm in `ui/display.rs`
4. Wire `Instruction` variants in `messages/instruction.rs`

### Adding a scan option
1. Add field to `ScanOptions` in `libdiskonaut/src/scan/mod.rs`
2. Thread it through **both** walkers: `scan/bulk.rs` (macOS) and the `fallback` module in
   `scan/mod.rs` (everywhere else). The fallback is `cfg`-selected away on macOS, so it is only
   ever run by its tests here — do not assume compiling it means it works.
3. Expose via CLI in `diskonaut/src/cli/mod.rs` and config if persistent
4. Add a `--benchmark` stage if it changes how the walk performs

### Touching filesystem attributes
Test against FAT as well as APFS — it is the filesystem that misreports. `docs/scan-performance.md`
has the FAT section, and the volume test is:

```bash
cargo test -p libdiskonaut --lib -- --ignored fat32
```

### Changing the scan
Read `docs/scan-performance.md` first. It records what was measured, what turned out not to
matter, and how to reproduce the numbers with `--benchmark`. The short version: the walk dominates
and the data model is free, so measure the walker before optimising anything else.

### Modifying treemap layout
- Core algorithm: `libdiskonaut/src/tiles/treemap.rs`
- Tile rendering: `diskonaut/src/ui/grid/`
- Adjust `HEIGHT_WIDTH_RATIO`, `MINIMUM_HEIGHT`, `MINIMUM_WIDTH` constants
- Entries below the minimum tile size are never dropped: they fold into the "small files" `x`
  marker, whose corner is clamped by `SMALL_FILES_MINIMUM_WIDTH/HEIGHT` so it stays visible even
  when the hidden entries round to zero cells

---

## CI Checks (must pass)

- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `cargo deny check`
- typos check

---

## File Size Reference

| File | Purpose |
|------|---------|
| `diskonaut/src/main.rs` | ~225 lines — thread/channel setup |
| `diskonaut/src/app/mod.rs` | ~300+ lines — core state machine |
| `libdiskonaut/src/tiles/board.rs` | ~200+ lines — tile nav/zoom |
| `libdiskonaut/src/tiles/treemap.rs` | ~150+ lines — squarify |
| `libdiskonaut/src/model/files/file_tree.rs` | ~150 lines — folder tree, hard-link accounting |
| `libdiskonaut/src/scan/bulk.rs` | ~800 lines — macOS `getattrlistbulk` walker |
| `diskonaut/src/bench/mod.rs` | ~230 lines — `--benchmark` harness |
