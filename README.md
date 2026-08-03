# diskonaut

**diskonaut** is an interactive terminal tool for exploring disk usage. Pick a directory, watch a live treemap fill in as files are scanned, drill into folders, and delete what you no longer need—without leaving the terminal.

## Features

- **Live scanning** — the treemap updates while the walk is still running
- **Treemap navigation** — proportional tiles for files and folders; zoom for dense directories
- **In-session cleanup** — delete files or folders and track space freed in the title bar
- **Apparent or on-disk size** — default shows blocks allocated on disk; `-a` uses logical file size
- **Unix-native** — Linux, macOS, and BSD; built on `ratatui` and parallel directory walking

## Requirements

- Linux/MacOS
- A terminal with reasonable size (roughly 50×15 cells minimum for the main UI)
- [Rust](https://www.rust-lang.org/tools/install)

## Configuration

Optional TOML config (see [example/config.toml](example/config.toml)):

- Default path: `~/.config/diskonaut/config.toml`
- Override path: `diskonaut -c /path/to/config.toml`

## Keyboard shortcuts

| Key                                | Action                                |
| ---------------------------------- | ------------------------------------- |
| `←` `→` `↑` `↓` or `h` `j` `k` `l` | Move selection                        |
| `Enter`                            | Open folder                           |
| `Esc`                              | Go to parent folder                   |
| `d`                                | Delete selected file or folder        |
| `+` / `-`                          | Zoom in / out                         |
| `0`                                | Reset zoom                            |
| `q` or `Ctrl+C`                    | Quit (confirm with `y` when prompted) |

Deletion always asks for `y` / `n` confirmation.
