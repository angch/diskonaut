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

## Benchmarking the scan

`--benchmark` scans headlessly and prints timings instead of starting the UI, so scanning strategies
can be compared on a real tree:

```sh
diskonaut --benchmark /                    # every stage, whole disk
diskonaut --benchmark --bench-stage pipeline ~/src
diskonaut --benchmark --max-depth 4 /      # partial scan, for a quick iteration loop
diskonaut --benchmark --threads 6 --bench-repeat 3 /
```

| Stage      | What it measures                                                       |
| ---------- | ---------------------------------------------------------------------- |
| `dua-walk` | the general-purpose `dua-core` walk alone                              |
| `dua-tree` | that walk feeding the folder tree                                      |
| `walk`     | the walk diskonaut uses now, alone                                     |
| `tree`     | that walk feeding the folder tree                                      |
| `pipeline` | scan and tree build on separate threads, exactly as the app runs them  |

Other flags: `--max-depth N` stops the descent (a partial scan), `--threads N` sets the worker
count, `--bench-repeat N` repeats each stage, `--single-thread` forces one worker.

On macOS the scan uses `getattrlistbulk(2)` directly, requesting only the name, type, flags, inode,
and one size field per entry — the general-purpose walker asks for the whole `stat` set and pays an
extra path lookup per directory. It also declines to enter mount points other than the scan root,
which is what keeps a scan of `/` from counting almost every file twice: macOS both mounts the data
volume at `/System/Volumes/Data` and grafts it into `/` through firmlinks.

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
