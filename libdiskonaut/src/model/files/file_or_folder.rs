use ::std::collections::{HashMap, VecDeque};
use ::std::ffi::{OsStr, OsString};
use ::std::path::{Path, PathBuf};

use crate::scan::{EntryMeta, NamedEntry};

#[derive(Debug, Clone)]
pub enum FileOrFolder {
    Folder(Folder),
    File(File),
}

impl FileOrFolder {
    pub fn size(&self) -> u128 {
        match self {
            FileOrFolder::Folder(folder) => folder.size,
            FileOrFolder::File(file) => file.size,
        }
    }
}

#[derive(Debug, Clone)]
pub struct File {
    pub name: OsString,
    pub size: u128,
}

#[derive(Debug, Clone)]
pub struct Folder {
    pub name: OsString,
    pub contents: HashMap<OsString, FileOrFolder>,
    pub size: u128,
    pub num_descendants: u64,
}

impl From<OsString> for Folder {
    fn from(name: OsString) -> Self {
        Folder {
            name,
            contents: HashMap::new(),
            size: 0,
            num_descendants: 0,
        }
    }
}
impl Folder {
    pub fn new(path: &Path) -> Self {
        let base_folder_name = path
            .iter()
            .next_back()
            .expect("could not get path base name");
        Self {
            name: base_folder_name.to_os_string(),
            contents: HashMap::new(),
            size: 0,
            num_descendants: 0,
        }
    }

    /// Insert an entry addressed by its path components relative to this folder.
    ///
    /// The descent is iterative and borrows each component, so inserting a file `n` levels deep
    /// costs `n` hash lookups and one name allocation for the levels that are new, rather than
    /// rebuilding a `PathBuf` suffix at every level as the recursive version did.
    pub fn add_entry<'a>(
        &mut self,
        meta: EntryMeta,
        relative_path: impl Iterator<Item = &'a OsStr>,
    ) {
        let size = u128::from(meta.size);
        let mut components = relative_path.peekable();
        let mut folder = self;

        while let Some(name) = components.next() {
            if !meta.is_dir {
                folder.size += size;
            }
            folder.num_descendants += 1;

            if components.peek().is_some() {
                folder = match folder
                    .contents
                    .entry(name.to_os_string())
                    .or_insert_with(|| FileOrFolder::Folder(Folder::from(name.to_os_string())))
                {
                    FileOrFolder::Folder(folder) => folder,
                    FileOrFolder::File(_) => unreachable!("got a file in the middle of a path"),
                };
            } else if meta.is_dir {
                // A directory can already exist here if one of its children was reported first.
                folder
                    .contents
                    .entry(name.to_os_string())
                    .or_insert_with(|| FileOrFolder::Folder(Folder::from(name.to_os_string())));
            } else {
                folder.contents.insert(
                    name.to_os_string(),
                    FileOrFolder::File(File {
                        name: name.to_os_string(),
                        size,
                    }),
                );
            }
        }
    }

    /// Add every entry of one directory, given that directory's path relative to this folder.
    pub fn add_dir_entries<'a>(
        &mut self,
        dir_path: impl Iterator<Item = &'a OsStr>,
        entries: &[NamedEntry],
    ) {
        let contained_size: u128 = entries
            .iter()
            .filter(|entry| !entry.meta.is_dir)
            .map(|entry| u128::from(entry.meta.size))
            .sum();
        let contained_count = entries.len() as u64;

        let mut folder = self;
        folder.size += contained_size;
        folder.num_descendants += contained_count;
        for name in dir_path {
            folder = match folder
                .contents
                .entry(name.to_os_string())
                .or_insert_with(|| FileOrFolder::Folder(Folder::from(name.to_os_string())))
            {
                FileOrFolder::Folder(folder) => folder,
                FileOrFolder::File(_) => unreachable!("got a file in the middle of a path"),
            };
            folder.size += contained_size;
            folder.num_descendants += contained_count;
        }

        for entry in entries {
            if entry.meta.is_dir {
                // The directory may already be here if its own contents were read first.
                folder
                    .contents
                    .entry(entry.name.clone())
                    .or_insert_with(|| FileOrFolder::Folder(Folder::from(entry.name.clone())));
            } else {
                folder.contents.insert(
                    entry.name.clone(),
                    FileOrFolder::File(File {
                        name: entry.name.clone(),
                        size: u128::from(entry.meta.size),
                    }),
                );
            }
        }
    }

    pub fn add_folder(&mut self, path: PathBuf) {
        self.add_entry(
            EntryMeta {
                size: 0,
                is_dir: true,
            },
            path.components().map(|component| component.as_os_str()),
        );
    }
    pub fn add_file(&mut self, path: PathBuf, size: u128) {
        self.add_entry(
            EntryMeta {
                size: size as u64,
                is_dir: false,
            },
            path.components().map(|component| component.as_os_str()),
        );
    }
    pub fn path(&self, mut folder_names: Vec<OsString>) -> Option<&FileOrFolder> {
        let next_folder_name = folder_names.remove(0);
        let next_in_path = &self.contents.get(&next_folder_name)?;
        if folder_names.is_empty() {
            Some(next_in_path)
        } else if let FileOrFolder::Folder(next_folder) = next_in_path {
            next_folder.path(folder_names)
        } else {
            Some(next_in_path)
        }
    }
    pub fn delete_path(&mut self, folder_names: &[OsString]) {
        // TODO: there are some needless allocations here, this is not terrible since
        // the deletion itself takes an order of magnitude longer, but it can be nice
        // to reduce them
        let mut folders_to_traverse: VecDeque<OsString> = VecDeque::from(folder_names.to_owned());
        if folder_names.len() == 1 {
            let name = folder_names
                .last()
                .expect("could not find last item in path");
            let removed_size = &self
                .contents
                .get(name)
                .expect("could not find folder")
                .size();
            let removed_descendents = match &self.contents.get(name).expect("could not find folder")
            {
                FileOrFolder::Folder(folder) => folder.num_descendants,
                FileOrFolder::File(_file) => 1,
            };
            self.size -= removed_size;
            self.num_descendants -= removed_descendents;
            self.contents.remove(name);
        } else {
            let (removed_size, removed_descendents) = {
                let item_to_remove = self
                    .path(Vec::from(folders_to_traverse.clone()))
                    .expect("could not find item to delete");
                let removed_size = item_to_remove.size();
                let removed_descendents = match item_to_remove {
                    FileOrFolder::Folder(folder) => folder.num_descendants,
                    FileOrFolder::File(_file) => 1,
                };
                (removed_size, removed_descendents)
            };
            let next_name = folders_to_traverse
                .pop_front()
                .expect("could not find next path folder");
            let next_item = &mut self
                .contents
                .get_mut(&next_name)
                .expect("could not find folder in path");
            match next_item {
                FileOrFolder::Folder(folder) => {
                    self.size -= removed_size;
                    self.num_descendants -= removed_descendents;
                    folder.delete_path(&Vec::from(folders_to_traverse));
                }
                FileOrFolder::File(_) => {
                    panic!("got a file in the middle of a path");
                }
            }
        }
    }
}
