//! Parallel directory traversal (`jwalk`).

use ::std::fs::Metadata;
use ::std::path::{Path, PathBuf};

use ::jwalk::Parallelism::{RayonDefaultPool, Serial};
use ::jwalk::WalkDir;

use crate::model::{FileTree, Folder};

/// Options controlling filesystem traversal.
#[derive(Clone, Copy, Debug)]
pub struct ScanOptions {
    /// Use a rayon thread pool for the walk.
    pub parallel: bool,
    /// Passed through to [`FileTree`] when using [`scan_into_tree`].
    pub show_apparent_size: bool,
    /// Skip hidden files and directories.
    pub skip_hidden: bool,
    /// Follow symbolic links.
    pub follow_links: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            parallel: true,
            show_apparent_size: false,
            skip_hidden: false,
            follow_links: false,
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
    let parallelism = if options.parallel {
        RayonDefaultPool
    } else {
        Serial
    };

    WalkDir::new(root.as_ref())
        .parallelism(parallelism)
        .skip_hidden(options.skip_hidden)
        .follow_links(options.follow_links)
        .into_iter()
        .map(|entry| match entry {
            Ok(entry) => match entry.metadata() {
                Ok(metadata) => ScanItem::Entry {
                    metadata,
                    path: entry.path().to_path_buf(),
                },
                Err(_) => ScanItem::ReadError,
            },
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
