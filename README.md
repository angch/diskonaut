# diskonaut

**diskonaut** is an interactive terminal tool for exploring disk usage. Pick a directory, watch a live treemap fill in as files are scanned, drill into folders, and delete what you no longer need—without leaving the terminal.

## Features

- **Live scanning** — the treemap updates while the walk is still running
- **Treemap navigation** — proportional tiles for files and folders; zoom for dense directories
- **In-session cleanup** — delete files or folders and track space freed in the title bar
- **Apparent or on-disk size** — default shows blocks allocated on disk; `-a` uses logical file size
- **Unix-native** — Linux, macOS, and BSD; built on `ratatui` and parallel directory walking

## Requirements

- A Unix-like system (Linux, macOS, or BSD)
- A terminal with reasonable size (roughly 50×15 cells minimum for the main UI)
- [Rust](https://www.rust-lang.org/tools/install) 1.85+ if building from source

Windows is not supported in **0.12.0**.

## Install

**From this repository:**

```bash
git clone https://github.com/Gigas002/diskonaut.git
cd diskonaut
cargo install --path diskonaut
```

The installable binary comes from the `diskonaut` crate; scanning and layout logic live in the sibling `libdiskonaut` library.

## Quick start

```bash
# Scan the current directory
diskonaut

# Scan a specific path
diskonaut ~/Downloads

# Show logical sizes (useful on compressed or sparse filesystems)
diskonaut -a /var/log

# Delete without a confirmation prompt (use with care)
diskonaut -x /tmp/staging
```

Run `diskonaut --help` for the full option list.

## Keyboard shortcuts

| Key | Action |
|-----|--------|
| `←` `→` `↑` `↓` or `h` `j` `k` `l` | Move selection |
| `Enter` | Open folder |
| `Esc` | Go to parent folder |
| `Backspace` | Delete selected file or folder |
| `+` / `-` | Zoom in / out |
| `0` | Reset zoom |
| `q` or `Ctrl+C` | Quit (confirm with `y` when prompted) |

Deletion always asks for `y` / `n` confirmation.

## Command-line options

```
diskonaut [OPTIONS] [FOLDER]

Arguments:
  FOLDER    Directory to scan (default: current working directory)

Options:
  -a, --apparent-size   Show file size instead of on-disk usage
  -h, --help            Print help
```

## Repository layout

```
diskonaut/          # workspace root
├── libdiskonaut/   # scan, file tree, treemap math, formatting
└── diskonaut/      # CLI + TUI
```

## Developing

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

CI also runs `typos`, `cargo deny check`, and `cargo doc`.

## Changelog

See [CHANGELOG.md](CHANGELOG.md). **0.12.0** is a major refresh: workspace split, Rust 2024, `ratatui` / `clap` / `rustix`, and removal of Windows support.

## License

MIT — see [LICENSE](LICENSE).
