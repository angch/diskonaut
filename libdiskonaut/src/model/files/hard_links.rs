use ::std::collections::HashMap;
use ::std::collections::hash_map::Entry;
use ::std::ffi::OsString;
use ::std::path::{Component, Path};

/// Directory components, relative to the scan root, of a folder holding a link to a file.
type Directory = Vec<OsString>;

/// A file reachable by more than one path, and where those paths were.
struct LinkedFile {
    size: u64,
    /// One entry per distinct folder holding a link, in the order they were found. A file's link
    /// count bounds this, and files with many links are rare, so it stays short in practice.
    directories: Vec<Directory>,
}

/// Charges each hard-linked file to any one folder at most once.
///
/// A folder's size answers "how much space is held under here", so the same blocks reached twice
/// within one folder must count once. Two links in sibling folders, though, each hold that space
/// on their own — while the parent they share still counts it once.
///
/// With `a/a`, `a/b` and `b/a` all links to the same 1 KiB file: `a` is 1 KiB, `b` is 1 KiB, and
/// the root containing both is 1 KiB.
#[derive(Default)]
pub struct HardLinks {
    files: HashMap<u64, LinkedFile>,
}

/// How many leading components two directories share.
fn shared_components(left: &[OsString], right: &Path) -> usize {
    left.iter()
        .zip(right.components().map(Component::as_os_str))
        .take_while(|(left, right)| left.as_os_str() == *right)
        .count()
}

impl HardLinks {
    /// Record a link to `inode`, of `size` bytes, found in `directory` (relative to the scan root).
    ///
    /// Returns the depth of the deepest folder that has already been charged for this file, so
    /// that the caller charges only the folders below it. `None` means no folder has been charged
    /// yet and the whole path should be.
    ///
    /// The answer is the longest prefix `directory` shares with *any* folder already holding a
    /// link, not with all of them at once: a folder is already charged exactly when it is an
    /// ancestor of some earlier link, and collapsing the earlier links into a single common
    /// ancestor would forget the deeper folders among them.
    pub fn charge(&mut self, inode: u64, size: u64, directory: &Path) -> Option<usize> {
        match self.files.entry(inode) {
            Entry::Occupied(mut seen) => {
                let seen = seen.get_mut();
                if seen.size != size {
                    // Inode numbers are unique only within a filesystem, and a scan of `/` on
                    // macOS covers a volume group whose volumes number their inodes separately.
                    // Two different sizes cannot be the same file, so charge this one in full.
                    return None;
                }
                let depth = directory.components().count();
                let mut deepest = 0;
                let mut already_listed = false;
                for existing in &seen.directories {
                    let shared = shared_components(existing, directory);
                    deepest = deepest.max(shared);
                    already_listed |= shared == depth && existing.len() == depth;
                }
                if !already_listed {
                    seen.directories.push(components_of(directory));
                }
                Some(deepest)
            }
            Entry::Vacant(slot) => {
                slot.insert(LinkedFile {
                    size,
                    directories: vec![components_of(directory)],
                });
                None
            }
        }
    }

    /// Number of distinct hard-linked files the scan has seen, for reporting and tests.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.files.len()
    }
}

fn components_of(directory: &Path) -> Directory {
    directory
        .components()
        .map(|component| component.as_os_str().to_os_string())
        .collect()
}
