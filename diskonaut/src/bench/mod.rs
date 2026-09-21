//! Headless scan benchmark (`--benchmark`).
//!
//! Runs the scan with no terminal attached and reports where the time goes. The stages nest, so
//! subtracting one from the next attributes cost to a layer:
//!
//! * `*-walk` — traversal alone, with entries dropped as they arrive.
//! * `*-tree` — traversal plus building the in-memory folder tree on the consuming thread.
//! * `pipeline` — traversal on worker threads and tree building on another, across the same
//!   channel the real app uses. This is what the app's loading time actually costs.
//!
//! The `dua-*` stages measure the general-purpose `dua-core` walker, the others the walker the app
//! now uses. Comparing them is the point: they scan the same tree, so the difference is the walker.

use ::std::path::Path;
use ::std::sync::mpsc::{self, Receiver, SyncSender};
use ::std::thread;
use ::std::time::{Duration, Instant};

use libdiskonaut::scan::thread_count;
use libdiskonaut::{
    DirEntries, FileTree, Folder, ScanItem, ScanOptions, scan_directories, scan_folder,
};

/// Which part of the scan pipeline to measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum BenchStage {
    /// The `dua-core` walk with nothing layered on top.
    DuaWalk,
    /// The `dua-core` walk feeding the folder tree.
    DuaTree,
    /// The current walk, entries counted but discarded.
    Walk,
    /// The current walk feeding the folder tree.
    Tree,
    /// The current walk and tree build on separate threads, exactly as the app runs them.
    Pipeline,
    /// Run every stage in order.
    All,
}

const ALL_STAGES: &[BenchStage] = &[
    BenchStage::DuaWalk,
    BenchStage::DuaTree,
    BenchStage::Walk,
    BenchStage::Tree,
    BenchStage::Pipeline,
];

/// Outcome of one benchmark run.
struct StageResult {
    stage: &'static str,
    elapsed: Duration,
    entries: u64,
    failed: u64,
    total_size: u128,
}

impl StageResult {
    fn report(&self) {
        let seconds = self.elapsed.as_secs_f64();
        let rate = if seconds > 0.0 {
            self.entries as f64 / seconds
        } else {
            0.0
        };
        println!(
            "{:<11}{:>8.3}s  {:>11} entries  {:>10.0} entries/s  {:>7} unreadable  {:>10}",
            self.stage,
            seconds,
            self.entries,
            rate,
            self.failed,
            human_size(self.total_size),
        );
    }
}

fn human_size(bytes: u128) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn new_tree(path: &Path) -> FileTree {
    FileTree::new(Folder::new(path), path.to_path_buf())
}

/// Finish a tree-building stage.
///
/// Dropping a multi-million-node tree takes longer than building it and is no part of the scan;
/// the app leaks it deliberately for the same reason.
fn finish(
    stage: &'static str,
    start: Instant,
    entries: u64,
    failed: u64,
    tree: FileTree,
) -> StageResult {
    let result = StageResult {
        stage,
        elapsed: start.elapsed(),
        entries,
        failed,
        total_size: tree.get_total_size(),
    };
    std::mem::forget(tree);
    result
}

/// The `dua-core` walk, optionally building the tree from it.
fn bench_dua(path: &Path, options: ScanOptions, build_tree: bool) -> StageResult {
    let start = Instant::now();
    let mut tree = new_tree(path);
    let mut entries = 0u64;
    let mut failed = 0u64;
    let mut total_size = 0u128;
    for item in scan_folder(path, options) {
        match item {
            ScanItem::Entry { meta, path } => {
                entries += 1;
                if build_tree {
                    tree.add_entry(meta, &path);
                } else {
                    total_size += u128::from(meta.size);
                }
            }
            ScanItem::ReadError => failed += 1,
        }
    }
    if build_tree {
        return finish("dua-tree", start, entries, failed, tree);
    }
    StageResult {
        stage: "dua-walk",
        elapsed: start.elapsed(),
        entries,
        failed,
        total_size,
    }
}

/// The current walk, optionally building the tree from it.
fn bench_scan(path: &Path, options: ScanOptions, build_tree: bool) -> StageResult {
    let start = Instant::now();
    let mut tree = new_tree(path);
    let mut entries = 0u64;
    let mut failed = 0u64;
    let mut total_size = 0u128;
    for directory in scan_directories(path, options) {
        entries += directory.entries.len() as u64;
        failed += directory.failed;
        if build_tree {
            tree.add_dir_entries(&directory.path, &directory.entries);
        } else {
            total_size += directory
                .entries
                .iter()
                .map(|entry| u128::from(entry.meta.size))
                .sum::<u128>();
        }
    }
    if build_tree {
        return finish("tree", start, entries, failed, tree);
    }
    StageResult {
        stage: "walk",
        elapsed: start.elapsed(),
        entries,
        failed,
        total_size,
    }
}

/// Number of entries batched into one channel message, matching the app.
const BATCH: usize = 4096;

/// Scan on worker threads and build the tree on another, across a channel, as the app does.
fn bench_pipeline(path: &Path, options: ScanOptions) -> StageResult {
    let start = Instant::now();
    let (sender, receiver): (SyncSender<Vec<DirEntries>>, Receiver<Vec<DirEntries>>) =
        mpsc::sync_channel(16);

    let scanner = thread::spawn({
        let path = path.to_path_buf();
        move || {
            let mut batch = Vec::new();
            let mut batched = 0usize;
            for directory in scan_directories(&path, options) {
                batched += directory.entries.len().max(1);
                batch.push(directory);
                if batched >= BATCH {
                    batched = 0;
                    if sender.send(std::mem::take(&mut batch)).is_err() {
                        return;
                    }
                }
            }
            let _ = sender.send(batch);
        }
    });

    let mut tree = new_tree(path);
    let mut entries = 0u64;
    let mut failed = 0u64;
    while let Ok(batch) = receiver.recv() {
        for directory in batch {
            entries += directory.entries.len() as u64;
            failed += directory.failed;
            tree.add_dir_entries(&directory.path, &directory.entries);
        }
    }
    let _ = scanner.join();

    finish("pipeline", start, entries, failed, tree)
}

/// Run the requested benchmark stages against `path` and print a report.
pub fn run(path: &Path, stage: BenchStage, options: ScanOptions, repeat: u32) {
    println!("benchmarking {}", path.display());
    println!(
        "  threads: {}   apparent-size: {}   max-depth: {}\n",
        thread_count(options),
        options.show_apparent_size,
        options
            .max_depth
            .map_or_else(|| "unlimited".to_string(), |depth| depth.to_string()),
    );

    let stages = match stage {
        BenchStage::All => ALL_STAGES,
        other => std::slice::from_ref(
            ALL_STAGES
                .iter()
                .find(|candidate| **candidate == other)
                .expect("every stage but `all` is listed in ALL_STAGES"),
        ),
    };

    for _ in 0..repeat.max(1) {
        for stage in stages {
            let result = match stage {
                BenchStage::DuaWalk => bench_dua(path, options, false),
                BenchStage::DuaTree => bench_dua(path, options, true),
                BenchStage::Walk => bench_scan(path, options, false),
                BenchStage::Tree => bench_scan(path, options, true),
                BenchStage::Pipeline | BenchStage::All => bench_pipeline(path, options),
            };
            result.report();
        }
    }
}
