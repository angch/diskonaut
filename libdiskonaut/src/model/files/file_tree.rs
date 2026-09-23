use ::std::ffi::{OsStr, OsString};
use ::std::path::{Component, Path, PathBuf};

use crate::model::{FileOrFolder, FileToDelete, Folder, HardLinks};
use ::std::sync::Arc;

use crate::scan::{DirEntries, DirSummary, EntryMeta, NamedEntry, SharedBlocks};

/// Shared-block sightings a tree has put off charging: one entry per directory that held any,
/// with the directory's path relative to the scan root.
type Sightings = Vec<(PathBuf, Vec<(SharedBlocks, u64)>)>;

pub struct FileTree {
    pub current_folder_names: Vec<OsString>,
    pub space_freed: u128,
    pub failed_to_read: u64,
    pub path_in_filesystem: PathBuf,
    base_folder: Folder,
    hard_links: HardLinks,
    /// Reused between calls: how much size to add at each depth from the base folder down.
    size_at_depth: Vec<u128>,
    /// When set, shared blocks are counted in full and noted here instead of being charged, so
    /// that several trees built in parallel can be merged and then reconciled once. `None` is the
    /// ordinary tree, which charges as it goes.
    deferred: Option<Sightings>,
}

impl FileTree {
    pub fn new(base_folder: Folder, path_in_filesystem: PathBuf) -> Self {
        let path_in_filesystem = path_in_filesystem
            .canonicalize()
            .unwrap_or(path_in_filesystem);
        FileTree {
            base_folder,
            current_folder_names: Vec::new(),
            path_in_filesystem,
            space_freed: 0,
            failed_to_read: 0,
            hard_links: HardLinks::default(),
            size_at_depth: Vec::new(),
            deferred: None,
        }
    }

    /// A tree that counts shared blocks in full and remembers where it saw them, for building in
    /// parallel. Call [`Self::replay_deferred`] on the merged result to make the sizes right.
    pub fn deferring_shared_blocks(base_folder: Folder, path_in_filesystem: PathBuf) -> Self {
        let mut tree = Self::new(base_folder, path_in_filesystem);
        tree.deferred = Some(Vec::new());
        tree
    }

    /// Fold another tree of the same scan root into this one.
    ///
    /// The other tree's folders add to this one's, and its deferred sightings — if either side
    /// has any — are carried over so that one `replay_deferred` on the result settles everything.
    pub fn merge_from(&mut self, other: FileTree) {
        self.base_folder.merge_from(other.base_folder);
        self.failed_to_read += other.failed_to_read;
        if let Some(theirs) = other.deferred {
            self.deferred.get_or_insert_with(Vec::new).extend(theirs);
        }
    }

    /// Charge every shared block this tree deferred, and take back what was over-counted.
    ///
    /// During a deferred build each shared entry was added in full to every ancestor. Charging it
    /// now says which of those ancestors — from the root down to some depth — had already counted
    /// the same blocks through another path; those give the size back. The ledger's answers are
    /// order-independent, so replaying in shard order rather than arrival order lands on the same
    /// per-folder sizes an ordinary tree would have. Afterwards the tree is an ordinary tree.
    pub fn replay_deferred(&mut self) {
        let Some(sightings) = self.deferred.take() else {
            return;
        };
        for (dir, shared) in sightings {
            let depth = dir.components().count();
            let dir_ref = self.hard_links.directory(&dir);
            for (blocks, size) in shared {
                if let Some(charged) = self.hard_links.charge_in(blocks, size, dir_ref) {
                    self.base_folder.subtract_along(
                        dir.components().map(Component::as_os_str),
                        charged.min(depth),
                        u128::from(size),
                    );
                }
            }
        }
    }
    pub fn get_total_size(&self) -> u128 {
        self.base_folder.size
    }
    pub fn get_total_descendants(&self) -> u64 {
        self.base_folder.num_descendants
    }
    pub fn get_current_folder(&self) -> &Folder {
        if self.current_folder_names.is_empty() {
            &self.base_folder
        } else if let Some(FileOrFolder::Folder(current_folder)) =
            self.base_folder.path(self.current_folder_names.clone())
        {
            current_folder
        } else {
            // here we have something in current_folder_names but the last
            // one is somehow not a folder... this is a corrupted state
            unreachable!("couldn't find current folder size")
        }
    }
    pub fn get_current_folder_size(&self) -> u128 {
        self.get_current_folder().size
    }
    pub fn get_current_path(&self) -> PathBuf {
        let mut full_path = PathBuf::from(&self.path_in_filesystem);
        for folder in &self.current_folder_names {
            full_path.push(folder)
        }
        full_path
    }
    pub fn item_in_current_folder(&self, item_name: &OsStr) -> Option<&FileOrFolder> {
        let current_folder = &self.get_current_folder();
        current_folder.path(vec![item_name.to_os_string()])
    }
    pub fn enter_folder(&mut self, folder_name: &OsStr) {
        self.current_folder_names.push(folder_name.to_os_string());
    }
    pub fn leave_folder(&mut self) -> bool {
        // true => succeeded, false => at base folder
        self.current_folder_names.pop().is_some()
    }
    pub fn delete_file(&mut self, file_to_delete: &FileToDelete) {
        let path_to_delete = &file_to_delete.path_to_file;
        self.base_folder.delete_path(path_to_delete);
    }
    /// How many distinct files the scan has seen under more than one name.
    pub fn hard_linked_files(&self) -> usize {
        self.hard_links.tracked()
    }
    /// How many distinct reflinked files the scan has seen, counted once per set of shared blocks.
    pub fn reflinked_files(&self) -> usize {
        self.hard_links.tracked_reflinks()
    }
    /// Add a directory's outline — its subfolders and its files' total — without its files.
    ///
    /// This is the live view while the real tree is built on other threads: cheap enough to run
    /// on the rendering thread for every directory as it is scanned, and enough to show every
    /// folder with a running size. Files appear when the finished tree takes this one's place.
    /// Shared blocks are counted in full here, so sizes can run a little high until then.
    pub fn add_summary(&mut self, summary: DirSummary) {
        let DirSummary {
            dirs,
            files_size,
            entries,
        } = summary;
        let (dir_path, names, dir_entries) = dirs.into_parts();
        let Ok(relative) = dir_path.strip_prefix(&self.path_in_filesystem) else {
            return;
        };
        let depth = relative.components().count();
        let file_count = entries.saturating_sub(dir_entries.len() as u64);
        self.size_at_depth.clear();
        self.size_at_depth.resize(depth + 1, u128::from(files_size));
        self.base_folder.add_dir_entries(
            relative.components().map(Component::as_os_str),
            names,
            dir_entries,
            &self.size_at_depth,
            file_count,
        );
    }

    /// Carry over where `other` had navigated to, if that folder exists here; back to the root
    /// if it does not.
    pub fn adopt_navigation_from(&mut self, other: &FileTree) {
        let names = other.current_folder_names.clone();
        let exists = names.is_empty()
            || matches!(
                self.base_folder.path(names.clone()),
                Some(FileOrFolder::Folder(_))
            );
        self.current_folder_names = if exists { names } else { Vec::new() };
        self.space_freed = other.space_freed;
    }

    /// Add every entry of one directory at once.
    ///
    /// Resolving `dir_path` is O(depth), and doing it once for the whole directory rather than
    /// once per entry is what keeps tree building off the critical path of a fast walk.
    pub fn add_dir_entries(&mut self, directory: DirEntries) {
        let (dir_path, names, entries) = directory.into_parts();
        // A directory from outside the scanned tree has no place in it. Silently folding such a
        // path into the base folder, as skipping a component count would, invents entries.
        let Ok(relative) = dir_path.strip_prefix(&self.path_in_filesystem) else {
            return;
        };
        self.add_relative_dir_entries(relative, names, entries);
    }
    pub fn add_entry(&mut self, meta: EntryMeta, entry_full_path: &Path) {
        let Ok(relative) = entry_full_path.strip_prefix(&self.path_in_filesystem) else {
            return;
        };
        let (Some(name), Some(parent)) = (relative.file_name(), relative.parent()) else {
            // The scan root itself, which is not an entry inside the tree.
            return;
        };
        let mut single = DirEntries::new(Arc::from(parent));
        single.push(name, meta);
        let (_, names, entries) = single.into_parts();
        self.add_relative_dir_entries(parent, names, entries);
    }
    /// Add one directory's entries, given that directory's path relative to the scan root.
    ///
    /// A hard-linked file is charged only to the folders that have not already counted it, which
    /// is what makes each folder's size the space actually held beneath it rather than the sum of
    /// its entries. See [`HardLinks`].
    fn add_relative_dir_entries(
        &mut self,
        relative_dir: &Path,
        names: Vec<u8>,
        entries: Vec<NamedEntry>,
    ) {
        let depth = relative_dir.components().count();
        let Self {
            base_folder,
            hard_links,
            size_at_depth,
            deferred,
            ..
        } = self;

        size_at_depth.clear();
        size_at_depth.resize(depth + 1, 0);
        let mut normal_size = 0u128;
        // Interned only if this directory turns out to hold a hard link; most do not.
        let mut this_dir = None;
        let mut noted: Option<Vec<(SharedBlocks, u64)>> = None;
        for entry in &entries {
            if entry.meta.is_dir {
                continue;
            }
            let size = u128::from(entry.meta.size);
            match entry.meta.shared_blocks() {
                Some(shared) if deferred.is_some() => {
                    // Counted in full for now like any other file; the sighting is kept so that
                    // `replay_deferred` can take back whatever turns out to be counted twice.
                    normal_size += size;
                    noted
                        .get_or_insert_with(Vec::new)
                        .push((shared, entry.meta.size));
                }
                Some(shared) => {
                    let dir = *this_dir.get_or_insert_with(|| hard_links.directory(relative_dir));
                    let charged_down_to = hard_links.charge_in(shared, entry.meta.size, dir);
                    let first_uncharged = charged_down_to.map_or(0, |charged| charged + 1);
                    for folder_size in &mut size_at_depth[first_uncharged.min(depth + 1)..] {
                        *folder_size += size;
                    }
                }
                None => normal_size += size,
            }
        }
        if let (Some(deferred), Some(noted)) = (deferred.as_mut(), noted) {
            deferred.push((relative_dir.to_path_buf(), noted));
        }
        if normal_size > 0 {
            for folder_size in &mut size_at_depth[..] {
                *folder_size += normal_size;
            }
        }

        base_folder.add_dir_entries(
            relative_dir.components().map(Component::as_os_str),
            names,
            entries,
            size_at_depth,
            0,
        );
    }
}
