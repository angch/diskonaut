use ::std::collections::VecDeque;
use ::std::ffi::{OsStr, OsString};
use ::std::os::unix::ffi::OsStrExt;
use ::std::path::{Path, PathBuf};

use crate::scan::{EntryMeta, NamedEntry};

/// What a folder holds, by name.
///
/// A `Folder` is boxed so that the far more numerous files do not each pay for a folder's size:
/// the map slot for a file is a name and a size rather than a name and an entire folder.
pub type ContentsMap = super::Contents;

#[derive(Debug, Clone)]
pub enum FileOrFolder {
    Folder(Box<Folder>),
    File(File),
}

impl FileOrFolder {
    pub fn size(&self) -> u128 {
        match self {
            FileOrFolder::Folder(folder) => folder.size,
            FileOrFolder::File(file) => u128::from(file.size),
        }
    }
}

/// A file, as the tree holds it.
///
/// `size` is a `u64` and not a `u128` deliberately. It is the single most repeated field in the
/// model — one per file, millions of them — and it decides the size of `FileOrFolder`, which is
/// what every slot in every folder costs. At `u128` the enum is 24 bytes; at `u64` it is 16. A
/// `u64` counts to 16 EiB, which no file and no volume reaches.
#[derive(Debug, Clone, Copy)]
pub struct File {
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct Folder {
    pub name: OsString,
    pub contents: ContentsMap,
    pub size: u128,
    pub num_descendants: u64,
}

impl From<OsString> for Folder {
    fn from(name: OsString) -> Self {
        Folder {
            name,
            contents: ContentsMap::default(),
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
            contents: ContentsMap::default(),
            size: 0,
            num_descendants: 0,
        }
    }

    /// Insert an entry addressed by its path components relative to this folder.
    pub fn add_entry(
        &mut self,
        meta: EntryMeta,
        relative_path: impl IntoIterator<Item = impl AsRef<OsStr>>,
    ) {
        let mut components = relative_path.into_iter().peekable();
        let mut folder = self;
        while let Some(name) = components.next() {
            let name = name.as_ref();
            let size = u128::from(meta.size);
            folder.size += size;
            folder.num_descendants += 1;
            if components.peek().is_some() {
                folder.contents.insert_if_absent(name, || {
                    FileOrFolder::Folder(Box::new(Folder::from(name.to_os_string())))
                });
                folder = match folder.contents.get_mut(name) {
                    Some(FileOrFolder::Folder(folder)) => folder,
                    _ => unreachable!("got a file in the middle of a path"),
                };
            } else if meta.is_dir {
                // A directory can already exist here if one of its children was reported first.
                folder.contents.insert_if_absent(name, || {
                    FileOrFolder::Folder(Box::new(Folder::from(name.to_os_string())))
                });
            } else {
                folder
                    .contents
                    .insert(name, FileOrFolder::File(File { size: meta.size }));
            }
        }
    }

    /// Add every entry of one directory, given that directory's path relative to this folder.
    ///
    /// `size_at_depth[k]` is added to the folder `k` levels down, so that a hard-linked file can
    /// be charged to some ancestors and not others. For a tree without hard links every element
    /// is the same total. See [`crate::model::HardLinks`].
    pub fn add_dir_entries<'a>(
        &mut self,
        dir_path: impl Iterator<Item = &'a OsStr>,
        names: Vec<u8>,
        entries: Vec<NamedEntry>,
        size_at_depth: &[u128],
    ) {
        let contained_count = entries.len() as u64;
        let size_at = |depth: usize| size_at_depth.get(depth).copied().unwrap_or(0);

        let mut folder = self;
        folder.size += size_at(0);
        folder.num_descendants += contained_count;
        for (depth, name) in dir_path.enumerate() {
            folder.contents.insert_if_absent(name, || {
                FileOrFolder::Folder(Box::new(Folder::from(name.to_os_string())))
            });
            folder = match folder.contents.get_mut(name) {
                Some(FileOrFolder::Folder(next)) => next,
                _ => unreachable!("got a file in the middle of a path"),
            };
            folder.size += size_at(depth + 1);
            folder.num_descendants += contained_count;
        }

        // The scan packed these names once; the folder takes that buffer rather than copying each
        // name out of it, so an entry's name is never allocated between the kernel and the tree.
        let shift = folder.contents.absorb_names(names);
        folder.contents.reserve(entries.len());
        for entry in entries {
            let range = entry.name_range();
            let offset = shift + u32::try_from(range.start).expect("names fit in 4 GiB");
            let len = u32::try_from(range.len()).expect("a name fits in 4 GiB");
            if entry.meta.is_dir {
                // The directory may already be here if its own contents were read first, which is
                // the only case that has to look before it writes.
                let folder_name =
                    OsStr::from_bytes(folder.contents.names_at(offset, len)).to_os_string();
                folder.contents.place(
                    offset,
                    len,
                    FileOrFolder::Folder(Box::new(Folder::from(folder_name))),
                    true,
                );
            } else {
                folder.contents.place(
                    offset,
                    len,
                    FileOrFolder::File(File {
                        size: entry.meta.size,
                    }),
                    false,
                );
            }
        }
    }

    pub fn add_folder(&mut self, path: PathBuf) {
        self.add_entry(
            EntryMeta {
                size: 0,
                is_dir: true,
                ..EntryMeta::default()
            },
            path.components().map(|component| component.as_os_str()),
        );
    }
    pub fn add_file(&mut self, path: PathBuf, size: u128) {
        self.add_entry(
            EntryMeta {
                size: u64::try_from(size).unwrap_or(u64::MAX),
                links: 1,
                is_dir: false,
                ..EntryMeta::default()
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
    /// How much an item contributed to each of its ancestors' `num_descendants`.
    ///
    /// A folder counts its own contents plus itself: every ancestor was incremented once for the
    /// folder's entry and once for each entry inside it.
    fn delete_path_removed_descendants(item: &FileOrFolder) -> u64 {
        match item {
            FileOrFolder::Folder(folder) => folder.num_descendants + 1,
            FileOrFolder::File(_) => 1,
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
            let removed_descendents = Self::delete_path_removed_descendants(
                self.contents.get(name).expect("could not find folder"),
            );
            // Saturating because a hard-linked file's size was charged to this folder only once
            // however many links it has here, so removing each link would otherwise underflow.
            self.size = self.size.saturating_sub(*removed_size);
            self.num_descendants = self.num_descendants.saturating_sub(removed_descendents);
            self.contents.remove(name);
        } else {
            let (removed_size, removed_descendents) = {
                let item_to_remove = self
                    .path(Vec::from(folders_to_traverse.clone()))
                    .expect("could not find item to delete");
                let removed_size = item_to_remove.size();
                (
                    removed_size,
                    Self::delete_path_removed_descendants(item_to_remove),
                )
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
                    self.size = self.size.saturating_sub(removed_size);
                    self.num_descendants = self.num_descendants.saturating_sub(removed_descendents);
                    folder.delete_path(&Vec::from(folders_to_traverse));
                }
                FileOrFolder::File(_) => {
                    panic!("got a file in the middle of a path");
                }
            }
        }
    }
}
