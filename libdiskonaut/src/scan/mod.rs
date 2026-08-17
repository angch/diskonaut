//! Parallel directory traversal (`dua-core`).

use ::std::fs::Metadata;
use ::std::num::NonZero;
use ::std::path::{Path, PathBuf};

use ::dua_core::{Order, walk};

use crate::model::{FileTree, Folder};

/// Options controlling filesystem traversal.
#[derive(Clone, Copy, Debug)]
pub struct ScanOptions {
    /// Use multiple threads for the walk.
    pub parallel: bool,
    /// Passed through to [`FileTree`] when using [`scan_into_tree`].
    pub show_apparent_size: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            parallel: true,
            show_apparent_size: false,
        }
    }
}

/// One step of a directory walk.
#[derive(Debug)]
pub enum ScanItem {
    Entry { metadata: Metadata, path: PathBuf },
    ReadError,
}

/// Walk `root` and yield each filesystem entry (or a read error marker).
pub fn scan_folder(root: impl AsRef<Path>, options: ScanOptions) -> impl Iterator<Item = ScanItem> {
    let threads = if options.parallel {
        std::thread::available_parallelism().map_or(1, NonZero::get)
    } else {
        1
    };

    walk(root.as_ref(), threads, Order::Completion, |_| true).map(|entry| match entry {
        Ok(entry) => {
            let path = entry.path();
            match entry.metadata {
                Ok(metadata) => ScanItem::Entry { path, metadata },
                Err(_) => ScanItem::ReadError,
            }
        }
        Err(_) => ScanItem::ReadError,
    })
}

/// Walk `root` and populate a [`FileTree`]. Returns the tree and a count of read failures.
pub fn scan_into_tree(root: impl AsRef<Path>, options: ScanOptions) -> (FileTree, u64) {
    let root_path = root.as_ref().to_path_buf();
    let mut tree = FileTree::new(
        Folder::new(root.as_ref()),
        root_path.clone(),
        options.show_apparent_size,
    );
    let mut failed_to_read = 0u64;

    for item in scan_folder(&root_path, options) {
        match item {
            ScanItem::Entry { metadata, path } => tree.add_entry(&metadata, &path),
            ScanItem::ReadError => failed_to_read += 1,
        }
    }

    (tree, failed_to_read)
}

#[cfg(test)]
mod tests;
