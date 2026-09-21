//! Parallel directory traversal (`dua-core`).

use ::std::ffi::OsString;
use ::std::num::NonZero;
use ::std::path::{Path, PathBuf};
use ::std::sync::Arc;

use ::dua_core::{Options, Order, walk};

use crate::model::{FileTree, Folder};

#[cfg(target_os = "macos")]
pub mod bulk;

/// Options controlling filesystem traversal.
#[derive(Clone, Copy, Debug)]
pub struct ScanOptions {
    /// Use multiple threads for the walk.
    pub parallel: bool,
    /// Override the worker count. `None` uses one thread per core (or one if `parallel` is off).
    pub threads: Option<usize>,
    /// Report logical file length rather than blocks allocated on disk.
    pub show_apparent_size: bool,
    /// Stop descending below this depth (the root is depth 0). `None` means no limit.
    pub max_depth: Option<usize>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            parallel: true,
            threads: None,
            show_apparent_size: false,
            max_depth: None,
        }
    }
}

/// The few metadata fields the disk-usage model actually needs, extracted during the walk.
///
/// Keeping this small matters: one of these is produced for every file on the volume and handed
/// across a channel to the tree builder, so the platform `Metadata` (which is an order of
/// magnitude larger) never travels with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryMeta {
    /// Size already resolved according to [`ScanOptions::show_apparent_size`].
    pub size: u64,
    pub is_dir: bool,
}

/// An entry named relative to the directory that contains it.
#[derive(Debug)]
pub struct NamedEntry {
    pub name: OsString,
    pub meta: EntryMeta,
}

/// Every entry of one directory, and the number of its entries that could not be read.
///
/// Directory-at-a-time delivery is what lets the tree builder resolve a parent once per directory
/// instead of once per file.
#[derive(Debug)]
pub struct DirEntries {
    pub path: Arc<Path>,
    pub entries: Vec<NamedEntry>,
    pub failed: u64,
}

/// Walk `root`, yielding the contents of one directory at a time.
///
/// On macOS this uses [`bulk`], which asks the kernel only for the attributes disk usage needs.
/// Elsewhere it groups the `dua-core` walk, which reports a directory's entries consecutively.
pub fn scan_directories(root: &Path, options: ScanOptions) -> impl Iterator<Item = DirEntries> {
    #[cfg(target_os = "macos")]
    {
        bulk::walk_bulk(
            root,
            thread_count(options),
            options.show_apparent_size,
            options.max_depth,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        fallback::group_by_directory(root, options)
    }
}

/// Compiled on every platform, though only used off macOS, so that it cannot rot unnoticed.
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod fallback {
    use super::{
        DirEntries, EntryMeta, NamedEntry, Options, Order, ScanOptions, entry_size, thread_count,
        walk,
    };
    use ::std::path::Path;
    use ::std::sync::Arc;

    /// Collect the `dua-core` walk into per-directory groups.
    ///
    /// `Order::Completion` emits a directory's entries consecutively, so grouping consecutive
    /// entries that share a parent recovers whole directories without buffering the whole walk.
    pub fn group_by_directory(
        root: &Path,
        options: ScanOptions,
    ) -> impl Iterator<Item = DirEntries> {
        let apparent = options.show_apparent_size;
        let max_depth = options.max_depth;
        let mut walk = walk(
            root,
            thread_count(options),
            Order::Completion,
            Options::default(),
            move |entry| max_depth.is_none_or(|max| entry.depth < max),
        );

        std::iter::from_fn(move || {
            let mut group: Option<DirEntries> = None;
            loop {
                let Some(entry) = walk.next() else {
                    return group;
                };
                let Ok(entry) = entry else {
                    match &mut group {
                        Some(group) => group.failed += 1,
                        None => {
                            return Some(DirEntries {
                                path: Arc::from(root),
                                entries: Vec::new(),
                                failed: 1,
                            });
                        }
                    }
                    continue;
                };
                let Ok(metadata) = &entry.metadata else {
                    continue;
                };
                let named = NamedEntry {
                    name: entry.file_name,
                    meta: EntryMeta {
                        size: entry_size(metadata, apparent),
                        is_dir: entry.file_type.is_dir(),
                    },
                };
                match &mut group {
                    Some(open) if open.path == entry.parent_path => open.entries.push(named),
                    Some(_) => {
                        let finished = group.replace(DirEntries {
                            path: entry.parent_path,
                            entries: vec![named],
                            failed: 0,
                        });
                        return finished;
                    }
                    None => {
                        group = Some(DirEntries {
                            path: entry.parent_path,
                            entries: vec![named],
                            failed: 0,
                        });
                    }
                }
            }
        })
    }
}

/// One step of a directory walk.
#[derive(Debug)]
pub enum ScanItem {
    Entry { meta: EntryMeta, path: PathBuf },
    ReadError,
}

pub fn thread_count(options: ScanOptions) -> usize {
    if let Some(threads) = options.threads {
        return threads.max(1);
    }
    if options.parallel {
        // Past a handful of workers the filesystem, not the CPU, is the limit: measured on a
        // 14-core machine, a whole-disk scan is fastest around six to eight threads and gets
        // steadily slower beyond that as the kernel spends its time on contention.
        std::thread::available_parallelism()
            .map_or(1, NonZero::get)
            .min(MAX_SCAN_THREADS)
    } else {
        1
    }
}

/// Worker cap for the scan, past which filesystem contention outweighs added parallelism.
const MAX_SCAN_THREADS: usize = 8;

/// Walk `root` and yield each filesystem entry (or a read error marker).
pub fn scan_folder(root: impl AsRef<Path>, options: ScanOptions) -> impl Iterator<Item = ScanItem> {
    let threads = thread_count(options);
    let apparent = options.show_apparent_size;
    let max_depth = options.max_depth;

    walk(
        root.as_ref(),
        threads,
        Order::Completion,
        Options::default(),
        move |entry| max_depth.is_none_or(|max| entry.depth < max),
    )
    .map(move |entry| match entry {
        Ok(entry) => {
            let path = entry.path();
            match entry.metadata {
                Ok(metadata) => ScanItem::Entry {
                    path,
                    meta: EntryMeta {
                        size: entry_size(&metadata, apparent),
                        is_dir: entry.file_type.is_dir(),
                    },
                },
                Err(_) => ScanItem::ReadError,
            }
        }
        Err(_) => ScanItem::ReadError,
    })
}

#[cfg(target_os = "macos")]
fn entry_size(metadata: &::dua_core::Metadata, apparent: bool) -> u64 {
    if apparent {
        metadata.len()
    } else {
        metadata.allocated_size()
    }
}

#[cfg(not(target_os = "macos"))]
fn entry_size(metadata: &::dua_core::Metadata, apparent: bool) -> u64 {
    use ::std::os::unix::fs::MetadataExt;
    if apparent {
        metadata.len()
    } else {
        crate::os::size_on_disk_fast(metadata)
    }
}

/// Walk `root` and populate a [`FileTree`]. Returns the tree and a count of read failures.
pub fn scan_into_tree(root: impl AsRef<Path>, options: ScanOptions) -> (FileTree, u64) {
    let root_path = root.as_ref().to_path_buf();
    let mut tree = FileTree::new(Folder::new(root.as_ref()), root_path.clone());
    let mut failed_to_read = 0u64;

    for directory in scan_directories(&root_path, options) {
        failed_to_read += directory.failed;
        tree.add_dir_entries(&directory.path, &directory.entries);
    }

    (tree, failed_to_read)
}

#[cfg(test)]
mod tests;
