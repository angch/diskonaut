use ::std::ffi::{OsStr, OsString};
use ::std::path::{Path, PathBuf};

use crate::model::{FileOrFolder, FileToDelete, Folder};
use crate::scan::{EntryMeta, NamedEntry};

pub struct FileTree {
    pub current_folder_names: Vec<OsString>,
    pub space_freed: u128,
    pub failed_to_read: u64,
    pub path_in_filesystem: PathBuf,
    base_folder: Folder,
    /// Component count of `path_in_filesystem`, cached so that every entry added during a scan
    /// does not re-walk the root path to find where its relative part begins.
    base_component_count: usize,
}

impl FileTree {
    pub fn new(base_folder: Folder, path_in_filesystem: PathBuf) -> Self {
        FileTree {
            base_folder,
            current_folder_names: Vec::new(),
            base_component_count: path_in_filesystem.components().count(),
            path_in_filesystem,
            space_freed: 0,
            failed_to_read: 0,
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
    /// Add every entry of one directory at once.
    ///
    /// Resolving `dir_path` is O(depth), and doing it once for the whole directory rather than
    /// once per entry is what keeps tree building off the critical path of a fast walk.
    pub fn add_dir_entries(&mut self, dir_path: &Path, entries: &[NamedEntry]) {
        let relative = dir_path
            .components()
            .skip(self.base_component_count)
            .map(|component| component.as_os_str());
        self.base_folder.add_dir_entries(relative, entries);
    }
    pub fn add_entry(&mut self, meta: EntryMeta, entry_full_path: &Path) {
        let relative = entry_full_path
            .components()
            .skip(self.base_component_count)
            .map(|component| component.as_os_str());
        self.base_folder.add_entry(meta, relative);
    }
}
