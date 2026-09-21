mod app;
mod bench;
mod cli;
mod config;
mod error;
mod input;
mod messages;
mod state;
mod ui;

use ::std::io;
use ::std::path::PathBuf;
use ::std::process;
use ::std::sync::Arc;
use ::std::sync::atomic::{AtomicBool, Ordering};
use ::std::sync::mpsc;
use ::std::sync::mpsc::{Receiver, SyncSender};
use ::std::thread::park_timeout;
use ::std::{thread, time};
use clap::Parser;
use cli::Opt;
use error::Error;
use libdiskonaut::{ScanOptions, scan_directories};

use ::ratatui::backend::Backend;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::Event as BackEvent;
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use app::{App, UiMode};
use config::DiskonautConfig;
use input::{TerminalEvents, needs_quit_delay};
use messages::{Event, Instruction, handle_events};

/// Number of scanned entries batched into one message to the UI thread.
///
/// A whole-disk scan produces millions of entries; a channel round-trip per entry, as this once
/// did, costs more than reading the filesystem.
const SCAN_BATCH_SIZE: usize = 4096;

fn main() {
    if let Err(err) = try_main() {
        println!("Error: {}", err);
        process::exit(2);
    }
}
fn get_stdout() -> io::Result<io::Stdout> {
    Ok(io::stdout())
}

fn try_main() -> Result<(), Error> {
    let opts = Opt::parse();

    let config_path = opts
        .config
        .clone()
        .or_else(config::default_config_path)
        .unwrap_or_else(|| PathBuf::from("config"));
    let diskonaut_config =
        DiskonautConfig::load(opts.config.as_deref()).map_err(|source| Error::Config {
            path: config_path.clone(),
            source,
        })?;
    let keybinds = diskonaut_config
        .keybinds()
        .map_err(|source| Error::Config {
            path: config_path,
            source,
        })?;
    let show_apparent_size = opts.apparent_size || diskonaut_config.base.apparent_size;

    if opts.benchmark {
        let folder = opts.resolve_folder()?;
        bench::run(
            &folder,
            opts.bench_stage,
            ScanOptions {
                parallel: !opts.single_thread,
                threads: opts.threads,
                show_apparent_size,
                max_depth: opts.max_depth,
            },
            opts.bench_repeat,
        );
        return Ok(());
    }

    match get_stdout() {
        Ok(stdout) => {
            enable_raw_mode()?;
            let terminal_backend = CrosstermBackend::new(stdout);
            let terminal_events = TerminalEvents {};
            let folder = opts.resolve_folder()?;
            start(
                terminal_backend,
                Box::new(terminal_events),
                folder,
                show_apparent_size,
                keybinds,
            );
        }
        Err(_) => return Err(Error::NoStdout),
    }
    disable_raw_mode()?;
    Ok(())
}

fn start<B>(
    terminal_backend: B,
    terminal_events: Box<dyn Iterator<Item = BackEvent> + Send>,
    path: PathBuf,
    show_apparent_size: bool,
    keybinds: config::Keybinds,
) where
    B: Backend + Send + 'static,
{
    let mut active_threads = vec![];

    let (event_sender, event_receiver): (SyncSender<Event>, Receiver<Event>) =
        mpsc::sync_channel(1);
    let (instruction_sender, instruction_receiver): (
        SyncSender<Instruction>,
        Receiver<Instruction>,
    ) = mpsc::sync_channel(100);

    let running = Arc::new(AtomicBool::new(true));
    let loaded = Arc::new(AtomicBool::new(false));

    // Constructed before any thread reads stdin: `App::new` clears the terminal, which
    // queries the cursor position by writing a CPR request and reading its reply straight off
    // stdin. If `stdin_handler` were already polling stdin for input events, it could steal that
    // reply out from under the query, leaving it blocked until the next real keypress arrived and
    // the screen showing nothing in the meantime.
    let mut app = App::new(terminal_backend, path.clone(), event_sender, keybinds.clone());

    active_threads.push(
        thread::Builder::new()
            .name("event_executer".to_string())
            .spawn({
                let instruction_sender = instruction_sender.clone();
                || handle_events(event_receiver, instruction_sender)
            })
            .unwrap(),
    );

    active_threads.push(
        thread::Builder::new()
            .name("stdin_handler".to_string())
            .spawn({
                let instruction_sender = instruction_sender.clone();
                let running = running.clone();
                let keybinds = keybinds.clone();
                move || {
                    for evt in terminal_events {
                        if let BackEvent::Resize(_x, _y) = evt {
                            let _ = instruction_sender.send(Instruction::ResetUiMode);
                            let _ = instruction_sender.send(Instruction::Render);
                            continue;
                        }

                        let delay =
                            matches!(&evt, BackEvent::Key(_)) && needs_quit_delay(&evt, &keybinds);
                        if instruction_sender.send(Instruction::Keypress(evt)).is_err() {
                            break;
                        }
                        if delay {
                            // not ideal, but works in a pinch
                            park_timeout(time::Duration::from_millis(100));
                            // if we don't wait, the app won't have time to quit
                            if !running.load(Ordering::Acquire) {
                                // sometimes ctrl-c doesn't shut down the app
                                // (eg. dismissing an error message)
                                // in order not to be aware of those particularities
                                // we check "running"
                                break;
                            }
                        }
                    }
                }
            })
            .unwrap(),
    );

    active_threads.push(
        thread::Builder::new()
            .name("hd_scanner".to_string())
            .spawn({
                let path = path.clone();
                let instruction_sender = instruction_sender.clone();
                let loaded = loaded.clone();
                move || {
                    let scan_options = ScanOptions {
                        parallel: true,
                        threads: None,
                        show_apparent_size,
                        max_depth: None,
                    };
                    let mut batch = Vec::new();
                    let mut batched_entries = 0usize;
                    'scanning: for directory in scan_directories(&path, scan_options) {
                        batched_entries += directory.entries.len().max(1);
                        batch.push(directory);
                        if batched_entries >= SCAN_BATCH_SIZE {
                            batched_entries = 0;
                            let full = std::mem::take(&mut batch);
                            if instruction_sender
                                .send(Instruction::AddScannedDirectories(full))
                                .is_err()
                            {
                                // if we fail to send an instruction here, this likely means the program has
                                // ended and we need to break this loop as well in order not to hang
                                break 'scanning;
                            }
                        }
                    }
                    if !batch.is_empty() {
                        let _ = instruction_sender.send(Instruction::AddScannedDirectories(batch));
                    }
                    let _ = instruction_sender.send(Instruction::StartUi);
                    loaded.store(true, Ordering::Release);
                }
            })
            .unwrap(),
    );

    active_threads.push(
        thread::Builder::new()
            .name("loading_loop".to_string())
            .spawn({
                let instruction_sender = instruction_sender.clone();
                let running = running.clone();
                move || {
                    while running.load(Ordering::Acquire) && !loaded.load(Ordering::Acquire) {
                        let _ = instruction_sender.send(Instruction::ToggleScanningVisualIndicator);
                        let _ = instruction_sender.send(Instruction::RenderAndUpdateBoard);
                        park_timeout(time::Duration::from_millis(100));
                    }
                }
            })
            .unwrap(),
    );

    app.start(instruction_receiver);
    running.store(false, Ordering::Release);

    for thread_handler in active_threads {
        thread_handler.join().unwrap();
    }
}
