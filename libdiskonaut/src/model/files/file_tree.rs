use ::std::ffi::{OsStr, OsString};
use ::std::path::{Component, Path, PathBuf};

use crate::model::{FileOrFolder, FileToDelete, Folder, HardLinks};
use crate::scan::{EntryMeta, NamedEntry};

pub struct FileTree {
    pub current_folder_names: Vec<OsString>,
    pub space_freed: u128,
    pub failed_to_read: u64,
    pub path_in_filesystem: PathBuf,
    base_folder: Folder,
    hard_links: HardLinks,
    /// Reused between calls: how much size to add at each depth from the base folder down.
    size_at_depth: Vec<u128>,
    /// Reused between calls by [`FileTree::add_entry`].
    single_entry: Vec<NamedEntry>,
}

impl FileTree {
    pub fn new(base_folder: Folder, path_in_filesystem: PathBuf) -> Self {
        FileTree {
            base_folder,
            current_folder_names: Vec::new(),
            path_in_filesystem,
            space_freed: 0,
            failed_to_read: 0,
            hard_links: HardLinks::default(),
            size_at_depth: Vec::new(),
            single_entry: Vec::new(),
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
    /// How many distinct hard-linked files the scan has seen.
    pub fn hard_linked_files(&self) -> usize {
        self.hard_links.tracked()
    }
    /// Add every entry of one directory at once.
    ///
    /// Resolving `dir_path` is O(depth), and doing it once for the whole directory rather than
    /// once per entry is what keeps tree building off the critical path of a fast walk.
    pub fn add_dir_entries(&mut self, dir_path: &Path, entries: &[NamedEntry]) {
        // A directory from outside the scanned tree has no place in it. Silently folding such a
        // path into the base folder, as skipping a component count would, invents entries.
        let Ok(relative) = dir_path.strip_prefix(&self.path_in_filesystem) else {
            return;
        };
        self.add_relative_dir_entries(relative, entries);
    }
    pub fn add_entry(&mut self, meta: EntryMeta, entry_full_path: &Path) {
        let Ok(relative) = entry_full_path.strip_prefix(&self.path_in_filesystem) else {
            return;
        };
        let (Some(name), Some(parent)) = (relative.file_name(), relative.parent()) else {
            // The scan root itself, which is not an entry inside the tree.
            return;
        };
        // Taken out of `self` so the shared path below can borrow the rest of it.
        let mut single = std::mem::take(&mut self.single_entry);
        single.clear();
        single.push(NamedEntry {
            name: name.to_os_string(),
            meta,
        });
        self.add_relative_dir_entries(parent, &single);
        self.single_entry = single;
    }
    /// Add one directory's entries, given that directory's path relative to the scan root.
    ///
    /// A hard-linked file is charged only to the folders that have not already counted it, which
    /// is what makes each folder's size the space actually held beneath it rather than the sum of
    /// its entries. See [`HardLinks`].
    fn add_relative_dir_entries(&mut self, relative_dir: &Path, entries: &[NamedEntry]) {
        let depth = relative_dir.components().count();
        let Self {
            base_folder,
            hard_links,
            size_at_depth,
            ..
        } = self;

        size_at_depth.clear();
        size_at_depth.resize(depth + 1, 0);
        for entry in entries {
            if entry.meta.is_dir {
                continue;
            }
            let size = u128::from(entry.meta.size);
            let charged_down_to = if entry.meta.is_hardlinked() {
                hard_links.charge(entry.meta.inode, entry.meta.size, relative_dir)
            } else {
                None
            };
            // Folders above and including the deepest one already charged keep their totals; the
            // ones below it are seeing this file for the first time.
            let first_uncharged = charged_down_to.map_or(0, |charged| charged + 1);
            for folder_size in &mut size_at_depth[first_uncharged.min(depth + 1)..] {
                *folder_size += size;
            }
        }

        base_folder.add_dir_entries(
            relative_dir.components().map(Component::as_os_str),
            entries,
            size_at_depth,
        );
    }
}
