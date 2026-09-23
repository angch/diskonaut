# diskonaut

**diskonaut** is an interactive terminal tool for exploring disk usage. Pick a directory, watch a live treemap fill in as files are scanned, drill into folders, and delete what you no longer need—without leaving the terminal.

## Features

- **Live scanning** — the treemap updates while the walk is still running
- **Treemap navigation** — proportional tiles for files and folders; zoom for dense directories
- **In-session cleanup** — delete files or folders and track space freed in the title bar
- **Apparent or on-disk size** — default shows blocks allocated on disk; `-a` uses logical file size
- **Hard-link aware** — a file reached by several names counts once in each folder that holds it
- **Unix-native** — Linux, macOS, and BSD; built on `ratatui` and parallel directory walking
- **Stays put on request** — `-x` keeps the scan on one filesystem, like `du -x`

## Requirements

- Linux/MacOS
- A terminal with reasonable size (roughly 50×15 cells minimum for the main UI)
- [Rust](https://www.rust-lang.org/tools/install)

## Hard links and folder sizes

A folder's size is the space held under it: each distinct file counted once, however many names
point at it. If `a/a`, `a/b` and `b/a` are all links to the same 1 KiB file, then `a` is 1 KiB,
`b` is 1 KiB, and the root holding both is 1 KiB — deleting either folder alone frees nothing, and
deleting both frees 1 KiB.

This differs from `du`, which deduplicates in traversal order and so charges whichever directory it
reached first, reporting nothing for the others.

Two things follow that are worth knowing:

- **Sizes do not add up where hard links are involved.** A folder can be smaller than the sum of
  the tiles inside it — each file tile shows that file's own size, while the folder shows the space
  it holds.
- **Deleting one link frees nothing** until the last link is gone, so the "space freed" figure is
  optimistic in that case. A rescan restores the true picture.

`docs/scan-performance.md` covers the details and the reasoning.

## Filesystems and mount points

By default the scan crosses mount points, like `du`. Pass `-x` / `--one-file-system` to keep it on
the filesystem the scan started on.

One thing is skipped either way: a mount point that leads back to the filesystem the scan started
on, because those files are already being counted by another path. On macOS that is
`/System/Volumes/Data`, which is both a mount point and grafted into `/` through firmlinks — follow
both and almost every file on the machine is counted twice.

On Windows the scan never follows a junction, a symbolic link or a volume mounted in a folder, so
it stays on the volume it started on with or without `-x`. OneDrive placeholders and other
reparse points that stand for real files are scanned as usual.

## Hard links on Windows

A file with several names (a hard link) is counted once per folder, however many of its names that
folder holds. Linux and macOS report each file's link count for free; Windows directory listings do
not, and asking costs a file open per file. So on Windows diskonaut tracks files by their NTFS/ReFS
file id instead, which costs memory rather than time — about 100 bytes a file.

By default only the places hard links are normally made are tracked: the Windows directory, Edge,
Docker and Git installs, and package stores (`node_modules`, pnpm, uv, `.venv`, `site-packages`).
A link made by hand elsewhere is counted once per name, which overstates rather than understates.
To track everywhere, pass `--hard-link-threshold BYTES` — every file at least that large is
tracked, wherever it is:

```sh
diskonaut --hard-link-threshold 1 C:\       # exact: every non-empty file
diskonaut --hard-link-threshold 1048576 D:\ # only files of 1 MiB and more
```

On one 2M-entry `C:\`, the default found all but 240 MB of 17 GB of double-counted links, for 90 MB
of memory; tracking every file was exact, for 200 MB and about a second more.

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
extra path lookup per directory.

The scan does not enter mount points other than the scan root, the same restriction `du -x`
applies. On macOS this is what keeps a scan of `/` from counting almost every file twice, since the
data volume is both mounted at `/System/Volumes/Data` and grafted into `/` through firmlinks
(firmlinks are still followed — they are the only route to what they point at). One consequence
worth knowing: pointing diskonaut at a directory that contains nothing but mount points, such as
`/Volumes`, reports nothing. Scan the volume itself instead.

`docs/scan-performance.md` has the measurements, the reasoning, and notes for repeating the
exercise on another platform.

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
