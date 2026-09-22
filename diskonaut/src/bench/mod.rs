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
    /// The folder tree alone: entries are collected first, untimed, then fed to the model.
    TreeOnly,
    /// The current walk and tree build on separate threads, exactly as the app runs them.
    Pipeline,
    /// The walk feeding several tree builders at once, by directory, merged and reconciled at
    /// the end. What a parallel model would cost before it is wired into the app.
    Sharded,
    /// Run every stage in order.
    All,
}

const ALL_STAGES: &[BenchStage] = &[
    BenchStage::DuaWalk,
    BenchStage::DuaTree,
    BenchStage::Walk,
    BenchStage::Tree,
    BenchStage::TreeOnly,
    BenchStage::Pipeline,
    BenchStage::Sharded,
];

/// Outcome of one benchmark run.
struct StageResult {
    stage: &'static str,
    elapsed: Duration,
    entries: u64,
    failed: u64,
    total_size: u128,
    /// Distinct hard-linked files, counted once however many names point at them.
    hard_linked: usize,
    /// Distinct reflinked files, counted once however many copies share their blocks.
    reflinked: usize,
}

impl StageResult {
    fn report(&self) {
        let seconds = self.elapsed.as_secs_f64();
        let rate = if seconds > 0.0 {
            self.entries as f64 / seconds
        } else {
            0.0
        };
        let hard_linked = if self.hard_linked > 0 {
            format!("  {} hard-linked", self.hard_linked)
        } else {
            String::new()
        };
        let reflinked = if self.reflinked > 0 {
            format!("  {} reflinked", self.reflinked)
        } else {
            String::new()
        };
        println!(
            "{:<11}{:>8.3}s  {:>11} entries  {:>10.0} entries/s  {:>7} unreadable  {:>10}{}",
            self.stage,
            seconds,
            self.entries,
            rate,
            self.failed,
            human_size(self.total_size),
            format_args!("{hard_linked}{reflinked}"),
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
        hard_linked: tree.hard_linked_files(),
        reflinked: tree.reflinked_files(),
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
        hard_linked: 0,
        reflinked: 0,
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
        entries += directory.len() as u64;
        failed += directory.failed;
        if build_tree {
            tree.add_dir_entries(directory);
        } else {
            total_size += directory
                .entries()
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
        hard_linked: 0,
        reflinked: 0,
    }
}

/// The tree build with the walk taken out of the measurement.
///
/// `walk` against `tree` cannot separate the two on Linux, because the consuming thread drives the
/// walk iterator and a slow consumer stalls the walk's workers. Collecting every directory first
/// and timing only the model answers "what would the scan cost if the walk were free", which is
/// the floor a faster walker can reach.
fn bench_tree_only(path: &Path, options: ScanOptions) -> StageResult {
    let directories: Vec<DirEntries> = scan_directories(path, options).collect();

    let start = Instant::now();
    let mut tree = new_tree(path);
    let mut entries = 0u64;
    let mut failed = 0u64;
    for directory in directories {
        entries += directory.len() as u64;
        failed += directory.failed;
        tree.add_dir_entries(directory);
    }

    finish("tree-only", start, entries, failed, tree)
}

/// Number of entries batched into one channel message, matching the app.
const BATCH: usize = 4096;

/// Scan on worker threads and build the tree on another, across a channel, as the app does.
fn bench_pipeline(path: &Path, options: ScanOptions) -> StageResult {
    let start = Instant::now();
    let (sender, receiver): (SyncSender<Vec<DirEntries>>, Receiver<Vec<DirEntries>>) =
        mpsc::sync_channel(64);

    let scanner = thread::spawn({
        let path = path.to_path_buf();
        move || {
            let mut batch = Vec::with_capacity(128);
            let mut batched = 0usize;
            for directory in scan_directories(&path, options) {
                batched += directory.len().max(1);
                batch.push(directory);
                if batched >= BATCH {
                    batched = 0;
                    let to_send = std::mem::replace(&mut batch, Vec::with_capacity(128));
                    if sender.send(to_send).is_err() {
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
            entries += directory.len() as u64;
            failed += directory.failed;
            tree.add_dir_entries(directory);
        }
    }
    let _ = scanner.join();

    finish("pipeline", start, entries, failed, tree)
}

/// Which builder a directory belongs to.
///
/// Hashing the first `depth` components of the path relative to the scan root, rather than the
/// whole path, is what keeps the merge cheap: every directory under one depth-`depth` prefix
/// lands in the same shard, so shards overlap only at folders *shallower* than that, and
/// everything deeper moves into the merged tree as a whole subtree. The price is balance — a
/// prefix is indivisible, so one huge subtree is one shard's problem. `depth == 0` hashes the
/// whole path, which balances perfectly and makes every ancestor a shared one.
fn shard_of(root: &Path, path: &Path, depth: usize, shards: usize) -> usize {
    use ::std::os::unix::ffi::OsStrExt;
    let relative = path.strip_prefix(root).unwrap_or(path);
    let depth = if depth == 0 { usize::MAX } else { depth };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for component in relative.components().take(depth) {
        for byte in component.as_os_str().as_bytes() {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        // A separator, so that `ab/c` and `a/bc` do not hash alike.
        hash = (hash ^ u64::from(b'/')).wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash % shards as u64) as usize
}

/// The walk feeding `shards` tree builders in parallel, then one merge and one reconciliation.
///
/// This is the design under test: every builder owns a private tree and never shares memory
/// with another, so there is nothing to race on; the cost of that is a merge of the overlapping
/// ancestors afterwards, plus replaying the shared-block sightings each builder deferred. The
/// three phases are timed separately on stderr, since the decision rests on the last two being
/// small.
fn bench_sharded(path: &Path, options: ScanOptions, shards: usize, depth: usize) -> StageResult {
    let shards = shards.max(1);
    let start = Instant::now();

    let mut senders = Vec::with_capacity(shards);
    let mut builders = Vec::with_capacity(shards);
    for _ in 0..shards {
        let (sender, receiver): (SyncSender<Vec<DirEntries>>, Receiver<Vec<DirEntries>>) =
            mpsc::sync_channel(64);
        senders.push(sender);
        let root = path.to_path_buf();
        builders.push(thread::spawn(move || {
            let mut tree = FileTree::deferring_shared_blocks(Folder::new(&root), root);
            while let Ok(batch) = receiver.recv() {
                for directory in batch {
                    tree.add_dir_entries(directory);
                }
            }
            tree
        }));
    }

    let mut outboxes: Vec<Vec<DirEntries>> = (0..shards).map(|_| Vec::new()).collect();
    let mut queued = 0usize;
    let mut entries = 0u64;
    let mut failed = 0u64;
    for directory in scan_directories(path, options) {
        entries += directory.len() as u64;
        failed += directory.failed;
        outboxes[shard_of(path, &directory.path, depth, shards)].push(directory);
        queued += 1;
        if queued >= 256 {
            queued = 0;
            for (outbox, sender) in outboxes.iter_mut().zip(&senders) {
                if !outbox.is_empty() {
                    let _ = sender.send(std::mem::take(outbox));
                }
            }
        }
    }
    for (outbox, sender) in outboxes.into_iter().zip(&senders) {
        if !outbox.is_empty() {
            let _ = sender.send(outbox);
        }
    }
    drop(senders);

    let mut trees: Vec<FileTree> = builders
        .into_iter()
        .map(|builder| builder.join().expect("a tree builder panicked"))
        .collect();
    let built = Instant::now();

    let mut tree = trees.pop().expect("at least one shard");
    for other in trees {
        tree.merge_from(other);
    }
    let merged = Instant::now();

    tree.replay_deferred();
    let replayed = Instant::now();

    eprintln!(
        "  sharded x{shards} depth {depth}: walk+build {:.3}s  merge {:.3}s  replay {:.3}s",
        (built - start).as_secs_f64(),
        (merged - built).as_secs_f64(),
        (replayed - merged).as_secs_f64(),
    );
    finish("sharded", start, entries, failed, tree)
}

/// Run the requested benchmark stages against `path` and print a report.
pub fn run(
    path: &Path,
    stage: BenchStage,
    options: ScanOptions,
    repeat: u32,
    shards: usize,
    shard_depth: usize,
) {
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
                BenchStage::TreeOnly => bench_tree_only(path, options),
                BenchStage::Pipeline | BenchStage::All => bench_pipeline(path, options),
                BenchStage::Sharded => bench_sharded(path, options, shards, shard_depth),
            };
            result.report();
        }
    }
}
