use ::std::collections::HashMap;
use ::std::collections::hash_map::Entry;
use ::std::hash::{BuildHasherDefault, Hasher};
use ::std::path::{Path, PathBuf};

#[derive(Default)]
struct U64Hasher(u64);

impl Hasher for U64Hasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self
                .0
                .wrapping_mul(0x517cc1b727220a95)
                .wrapping_add(b as u64);
        }
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        let mut x = i.wrapping_mul(0xff51afd7ed558ccd);
        x ^= x >> 32;
        self.0 = x;
    }
}

type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<U64Hasher>>;

/// A file reachable by more than one path, and where those paths were.
struct LinkedFile {
    size: u64,
    /// One entry per distinct folder holding a link, in the order they were found.
    /// Stores the path relative to scan root, and its depth.
    directories: Vec<(PathBuf, usize)>,
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
    files: FastMap<u64, LinkedFile>,
}

/// How many leading components two directories share.
fn shared_components(left: &Path, right: &Path) -> usize {
    left.components()
        .zip(right.components())
        .take_while(|(l, r)| l == r)
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
        self.charge_with_depth(inode, size, directory, directory.components().count())
    }

    /// Record a link with a known `directory` depth, avoiding recount on repeated links in the same folder.
    pub fn charge_with_depth(
        &mut self,
        inode: u64,
        size: u64,
        directory: &Path,
        depth: usize,
    ) -> Option<usize> {
        match self.files.entry(inode) {
            Entry::Occupied(mut seen) => {
                let seen = seen.get_mut();
                if seen.size != size {
                    // Inode numbers are unique only within a filesystem, and a scan of `/` on
                    // macOS covers a volume group whose volumes number their inodes separately.
                    // Two different sizes cannot be the same file, so charge this one in full.
                    return None;
                }
                let mut deepest = 0;
                for (existing, existing_depth) in &seen.directories {
                    if existing.as_path() == directory {
                        return Some(*existing_depth);
                    }
                    let shared = shared_components(existing, directory);
                    deepest = deepest.max(shared);
                }
                seen.directories.push((directory.to_path_buf(), depth));
                Some(deepest)
            }
            Entry::Vacant(slot) => {
                slot.insert(LinkedFile {
                    size,
                    directories: vec![(directory.to_path_buf(), depth)],
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
