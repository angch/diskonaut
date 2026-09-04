# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
