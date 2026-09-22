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
    /// Do not cross filesystem boundaries (like `du -x`).
    ///
    /// The scan always declines to enter a filesystem it is already walking by another path,
    /// whatever this is set to, since that would count the same files twice.
    pub one_file_system: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            parallel: true,
            threads: None,
            show_apparent_size: false,
            max_depth: None,
            one_file_system: false,
        }
    }
}

/// The few metadata fields the disk-usage model actually needs, extracted during the walk.
///
/// Keeping this small matters: one of these is produced for every file on the volume and handed
/// across a channel to the tree builder, so the platform `Metadata` (which is an order of
/// magnitude larger) never travels with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntryMeta {
    /// Size already resolved according to [`ScanOptions::show_apparent_size`].
    pub size: u64,
    /// Filesystem identity, used to charge a hard-linked file to a folder only once.
    pub inode: u64,
    /// Directory entries pointing at this file; `1` for an ordinary file.
    pub links: u64,
    pub is_dir: bool,
}

impl EntryMeta {
    /// Whether more than one directory entry points at this file, so that the same blocks can be
    /// reached by more than one path and must not be counted twice within a folder.
    #[must_use]
    pub fn is_hardlinked(&self) -> bool {
        !self.is_dir && self.links > 1
    }
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
            options.one_file_system,
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
        DirEntries, EntryMeta, NamedEntry, Options, Order, ScanOptions, descend_predicate,
        entry_identity, entry_size, thread_count, walk,
    };
    use ::std::path::Path;
    use ::std::sync::Arc;

    /// Collect the `dua-core` walk into per-directory groups.
    ///
    /// `Order::Completion` reports a directory's entries together, so accumulating entries that
    /// share a parent recovers whole directories without buffering the whole walk. A directory
    /// whose entries arrive in several chunks simply yields several groups, which the tree builder
    /// handles: each group carries only its own entries' sizes and counts.
    pub fn group_by_directory(
        root: &Path,
        options: ScanOptions,
    ) -> impl Iterator<Item = DirEntries> {
        let apparent = options.show_apparent_size;
        let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let root: Arc<Path> = Arc::from(root_canon.as_path());
        let descend = descend_predicate(&root, options);
        let mut walk = walk(
            &root,
            thread_count(options),
            Order::Completion,
            Options::default(),
            descend,
        );

        // Held across calls: the group being accumulated is only yielded once an entry for a
        // different directory shows up, or the walk ends.
        let mut open: Option<DirEntries> = None;

        std::iter::from_fn(move || {
            loop {
                let Some(entry) = walk.next() else {
                    return open.take();
                };
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => {
                        // No entry, so nothing identifies which directory this belongs to.
                        match &mut open {
                            Some(open) => open.failed += 1,
                            None => {
                                return Some(DirEntries {
                                    path: Arc::clone(&root),
                                    entries: Vec::new(),
                                    failed: 1,
                                });
                            }
                        }
                        continue;
                    }
                };
                let ::dua_core::Entry {
                    depth,
                    file_name,
                    file_type,
                    metadata,
                    parent_path,
                    ..
                } = entry;
                if depth == 0 {
                    // The walk root itself, reported with the root's *parent* as its parent path.
                    // It is not an entry inside the tree being scanned.
                    continue;
                }
                let named = metadata.ok().map(|metadata| {
                    let (inode, links) = entry_identity(&metadata);
                    NamedEntry {
                        name: file_name,
                        meta: EntryMeta {
                            size: entry_size(&metadata, apparent),
                            inode,
                            links,
                            is_dir: file_type.is_dir(),
                        },
                    }
                });

                match &mut open {
                    Some(open)
                        if Arc::ptr_eq(&open.path, &parent_path) || open.path == parent_path =>
                    {
                        match named {
                            Some(named) => open.entries.push(named),
                            None => open.failed += 1,
                        }
                    }
                    // A different directory: start its group, and hand back the finished one.
                    // The new group already holds this entry, so nothing is lost by returning.
                    _ => {
                        let failed = u64::from(named.is_none());
                        let mut entries = Vec::with_capacity(32);
                        if let Some(named) = named {
                            entries.push(named);
                        }
                        let finished = open.replace(DirEntries {
                            path: parent_path,
                            entries,
                            failed,
                        });
                        if let Some(finished) = finished {
                            return Some(finished);
                        }
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
    let root = root
        .as_ref()
        .canonicalize()
        .unwrap_or_else(|_| root.as_ref().to_path_buf());
    let threads = thread_count(options);
    let apparent = options.show_apparent_size;
    let descend = descend_predicate(&root, options);

    walk(
        &root,
        threads,
        Order::Completion,
        Options::default(),
        descend,
    )
    .map(move |entry| match entry {
        Ok(entry) => {
            let path = entry.path();
            match entry.metadata {
                Ok(metadata) => {
                    let (inode, links) = entry_identity(&metadata);
                    ScanItem::Entry {
                        path,
                        meta: EntryMeta {
                            size: entry_size(&metadata, apparent),
                            inode,
                            links,
                            is_dir: entry.file_type.is_dir(),
                        },
                    }
                }
                Err(_) => ScanItem::ReadError,
            }
        }
        Err(_) => ScanItem::ReadError,
    })
}

/// Whether the walk should descend into a directory entry.
///
/// Unlike the native macOS walker, `dua-core` decides this from the entry as its parent listed it,
/// which is enough for a device comparison: on platforms using this path a mount point really does
/// report the mounted filesystem's device.
fn descend_predicate(
    root: &Path,
    options: ScanOptions,
) -> impl Fn(&::dua_core::Entry) -> bool + Send + Sync + 'static {
    let max_depth = options.max_depth;
    let root_device = options.one_file_system.then(|| {
        ::std::fs::metadata(root)
            .map(|metadata| ::std::os::unix::fs::MetadataExt::dev(&metadata))
            .unwrap_or_default()
    });
    move |entry| {
        if !max_depth.is_none_or(|max| entry.depth < max) {
            return false;
        }
        match (root_device, &entry.metadata) {
            (Some(root_device), Ok(metadata)) => entry_device(metadata) == root_device,
            _ => true,
        }
    }
}

/// The filesystem an entry lives on, however the platform's metadata spells it.
#[cfg(target_os = "macos")]
fn entry_device(metadata: &::dua_core::Metadata) -> u64 {
    metadata.dev()
}

#[cfg(not(target_os = "macos"))]
fn entry_device(metadata: &::dua_core::Metadata) -> u64 {
    use ::std::os::unix::fs::MetadataExt;
    metadata.dev()
}

/// Inode number and link count, however the platform's metadata spells them.
#[cfg(target_os = "macos")]
fn entry_identity(metadata: &::dua_core::Metadata) -> (u64, u64) {
    (metadata.ino(), metadata.nlink())
}

#[cfg(not(target_os = "macos"))]
fn entry_identity(metadata: &::dua_core::Metadata) -> (u64, u64) {
    use ::std::os::unix::fs::MetadataExt;
    (metadata.ino(), metadata.nlink())
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
    if apparent {
        metadata.len()
    } else {
        crate::os::size_on_disk_fast(metadata)
    }
}

/// Walk `root` and populate a [`FileTree`]. Returns the tree and a count of read failures.
pub fn scan_into_tree(root: impl AsRef<Path>, options: ScanOptions) -> (FileTree, u64) {
    let root_path = root
        .as_ref()
        .canonicalize()
        .unwrap_or_else(|_| root.as_ref().to_path_buf());
    let mut tree = FileTree::new(Folder::new(&root_path), root_path.clone());
    let mut failed_to_read = 0u64;

    for directory in scan_directories(&root_path, options) {
        failed_to_read += directory.failed;
        tree.add_dir_entries(&directory.path, directory.entries);
    }

    (tree, failed_to_read)
}

#[cfg(test)]
mod tests;
