//! Parallel directory traversal (`dua-core`).

use ::std::ffi::OsStr;
use ::std::num::NonZero;
use ::std::os::unix::ffi::OsStrExt;
use ::std::path::{Path, PathBuf};
use ::std::sync::Arc;

use ::dua_core::{Options, Order, walk};

use crate::model::{FileTree, Folder};

#[cfg(target_os = "macos")]
pub mod bulk;

#[cfg(target_os = "linux")]
pub mod linux;

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
    /// An identity for this file's blocks when every one of them is shared with another file
    /// through copy-on-write (XFS or btrfs reflink). `0` when they are not, or were not looked
    /// for. Two files sharing only part of themselves get `0` each and are counted in full.
    ///
    /// Reflinked files are the hard-link problem wearing a different hat: one set of blocks
    /// reachable by several paths. `links` does not see them — every copy is its own inode with
    /// `nlink == 1` — so without this the same blocks are counted once per copy.
    pub shared_extent: u64,
}

/// Why a file's blocks might already have been counted elsewhere.
///
/// Both cases mean the same thing to the model — these blocks can be reached by more than one
/// path, so a folder that reaches them twice must count them once — but they are identified
/// differently and must not be confused: an inode number and a physical block offset are
/// unrelated numbers that would otherwise collide in one map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedBlocks {
    /// Several names for one inode: a hard link.
    Inode(u64),
    /// Several inodes sharing one physical extent: a copy-on-write reflink.
    Extent(u64),
}

impl EntryMeta {
    /// Whether more than one directory entry points at this file, so that the same blocks can be
    /// reached by more than one path and must not be counted twice within a folder.
    #[must_use]
    pub fn is_hardlinked(&self) -> bool {
        !self.is_dir && self.links > 1
    }

    /// How this file's blocks are shared, if they are.
    ///
    /// A reflinked file may also be hard-linked. The extent identity is the stronger of the two —
    /// every hard link to a file reports the same first extent — so it is preferred, which keeps
    /// one file to one identity and one ledger entry.
    #[must_use]
    pub fn shared_blocks(&self) -> Option<SharedBlocks> {
        if self.is_dir {
            return None;
        }
        if self.shared_extent != 0 {
            return Some(SharedBlocks::Extent(self.shared_extent));
        }
        if self.links > 1 {
            return Some(SharedBlocks::Inode(self.inode));
        }
        None
    }
}

/// An entry named relative to the directory that contains it.
///
/// The name is not here: it lives in the owning [`DirEntries`]' packed buffer, and this records
/// where. An `OsString` per entry costs 24 bytes wherever it is stored plus a heap block of its
/// own, and a whole-volume scan makes millions of them only to hand them straight to the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedEntry {
    offset: u32,
    len: u32,
    pub meta: EntryMeta,
}

impl NamedEntry {
    /// Where this entry's name sits in the buffer that owns it.
    #[must_use]
    pub fn name_range(&self) -> ::std::ops::Range<usize> {
        let start = self.offset as usize;
        start..start + self.len as usize
    }
}

/// Every entry of one directory, and the number of its entries that could not be read.
///
/// Directory-at-a-time delivery is what lets the tree builder resolve a parent once per directory
/// instead of once per file. The names are concatenated into one buffer rather than allocated
/// individually, so a directory costs one allocation for all of its names and the tree can take
/// that buffer over without copying it.
#[derive(Debug)]
pub struct DirEntries {
    pub path: Arc<Path>,
    /// Every entry's name, end to end, in entry order.
    names: Vec<u8>,
    entries: Vec<NamedEntry>,
    pub failed: u64,
}

impl DirEntries {
    #[must_use]
    pub fn new(path: Arc<Path>) -> Self {
        Self {
            path,
            names: Vec::new(),
            entries: Vec::new(),
            failed: 0,
        }
    }

    /// A directory known to hold about `entries` entries and `name_bytes` of names between them.
    #[must_use]
    pub fn with_capacity(path: Arc<Path>, entries: usize, name_bytes: usize) -> Self {
        Self {
            path,
            names: Vec::with_capacity(name_bytes),
            entries: Vec::with_capacity(entries),
            failed: 0,
        }
    }

    /// Append an entry, copying its name into the buffer.
    ///
    /// # Panics
    ///
    /// If one directory's names exceed 4 GiB, which no filesystem permits.
    pub fn push(&mut self, name: &OsStr, meta: EntryMeta) {
        let bytes = name.as_bytes();
        let offset = u32::try_from(self.names.len()).expect("a directory's names fit in 4 GiB");
        let len = u32::try_from(bytes.len()).expect("a name fits in 4 GiB");
        self.names.extend_from_slice(bytes);
        self.entries.push(NamedEntry { offset, len, meta });
    }

    /// The name of an entry belonging to this directory.
    #[must_use]
    pub fn name(&self, entry: &NamedEntry) -> &OsStr {
        OsStr::from_bytes(&self.names[entry.name_range()])
    }

    #[must_use]
    pub fn entries(&self) -> &[NamedEntry] {
        &self.entries
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry, as a name and its metadata.
    pub fn iter(&self) -> impl Iterator<Item = (&OsStr, &EntryMeta)> {
        self.entries.iter().map(|entry| {
            (
                OsStr::from_bytes(&self.names[entry.name_range()]),
                &entry.meta,
            )
        })
    }

    /// Give back the slack both vectors grew while the directory was being read.
    ///
    /// They are filled by pushing, so they double as they go and end up around half again as large
    /// as they need to be — and the tree keeps them for the life of the scan, so that slack is
    /// permanent. One realloc per directory buys it back.
    pub fn shrink(&mut self) {
        self.names.shrink_to_fit();
        self.entries.shrink_to_fit();
    }

    /// Hand over the name buffer and the entries, so the tree can take the buffer rather than
    /// copy out of it.
    #[must_use]
    pub fn into_parts(self) -> (Arc<Path>, Vec<u8>, Vec<NamedEntry>) {
        (self.path, self.names, self.entries)
    }
}

/// Walk `root`, yielding the contents of one directory at a time.
///
/// On macOS this uses [`bulk`], which asks the kernel only for the attributes disk usage needs.
/// On Linux it uses [`linux`], which owns its own thread pool because `dua-core`'s stops scaling
/// well before the kernel does. Elsewhere it groups the `dua-core` walk, which reports a
/// directory's entries consecutively.
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
    #[cfg(target_os = "linux")]
    {
        linux::walk_linux(root, thread_count(options), options)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        fallback::group_by_directory(root, options)
    }
}

/// Compiled on every platform, though only selected on platforms that are neither macOS nor
/// Linux, so that it cannot rot unnoticed. The tests call it directly everywhere.
#[cfg_attr(any(target_os = "macos", target_os = "linux"), allow(dead_code))]
mod fallback {
    use super::{
        DirEntries, EntryMeta, Options, Order, ScanOptions, descend_predicate, dua_thread_count,
        entry_identity, entry_size, walk,
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
            dua_thread_count(options),
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
                                let mut lost = DirEntries::new(Arc::clone(&root));
                                lost.failed = 1;
                                return Some(lost);
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
                    EntryMeta {
                        size: entry_size(&metadata, apparent),
                        inode,
                        links,
                        is_dir: file_type.is_dir(),
                        shared_extent: 0,
                    }
                });

                match &mut open {
                    Some(open)
                        if Arc::ptr_eq(&open.path, &parent_path) || open.path == parent_path =>
                    {
                        match named {
                            Some(meta) => open.push(&file_name, meta),
                            None => open.failed += 1,
                        }
                    }
                    // A different directory: start its group, and hand back the finished one.
                    // The new group already holds this entry, so nothing is lost by returning.
                    _ => {
                        let mut group = DirEntries::with_capacity(parent_path, 32, 32 * 32);
                        match named {
                            Some(meta) => group.push(&file_name, meta),
                            None => group.failed = 1,
                        }
                        let finished = open.replace(group);
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

/// Workers for the walker the app actually uses.
pub fn thread_count(options: ScanOptions) -> usize {
    capped(options, MAX_SCAN_THREADS)
}

/// Workers for the `dua-core` walk, wherever it is still reached.
///
/// It needs its own number. The cap below is a property of a walker, not of a machine: raising the
/// native walker's cap to 24 and letting `dua-core` inherit it made the `dua-*` benchmark stages —
/// and `scan_folder`, which is public — 38% slower on this box, by running the one walker that
/// collapses past eight workers at 24 of them.
fn dua_thread_count(options: ScanOptions) -> usize {
    capped(options, MAX_DUA_THREADS)
}

fn capped(options: ScanOptions, cap: usize) -> usize {
    if let Some(threads) = options.threads {
        return threads.max(1);
    }
    if options.parallel {
        std::thread::available_parallelism()
            .map_or(1, NonZero::get)
            .min(cap)
    } else {
        1
    }
}

/// Worker cap for the scan, past which more workers stop paying for themselves.
///
/// The two numbers come from different walkers and do not transfer to each other.
///
/// On macOS the `getattrlistbulk` walk really does contend: a whole-disk scan is fastest around
/// six to eight workers on a 14-core machine and gets steadily slower beyond that.
///
/// On Linux the old cap of eight was a property of the `dua-core` walk, which collapsed past it —
/// not of the kernel, which serves sixteen concurrent walkers at near-linear throughput. With the
/// native walker the curve is flat from twelve workers up: on a 32-core box, XFS and ext4 both
/// bottom out around twenty-four and give up only a few percent by thirty-two.
#[cfg(target_os = "linux")]
const MAX_SCAN_THREADS: usize = 24;
#[cfg(not(target_os = "linux"))]
const MAX_SCAN_THREADS: usize = 8;

/// Worker cap for the `dua-core` walk, which collapses past eight on every machine measured.
const MAX_DUA_THREADS: usize = 8;

/// Walk `root` and yield each filesystem entry (or a read error marker).
pub fn scan_folder(root: impl AsRef<Path>, options: ScanOptions) -> impl Iterator<Item = ScanItem> {
    let root = root
        .as_ref()
        .canonicalize()
        .unwrap_or_else(|_| root.as_ref().to_path_buf());
    let threads = dua_thread_count(options);
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
                            shared_extent: 0,
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
        tree.add_dir_entries(directory);
    }

    (tree, failed_to_read)
}

#[cfg(test)]
mod tests;
