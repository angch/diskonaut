//! Core disk usage model, treemap layout, directory scanning, and helpers (no TUI).

pub mod format;
pub mod model;
pub mod os;
pub mod scan;
pub mod tiles;

pub use format::{DisplaySize, DisplaySizeRounded, truncate_end, truncate_middle};
pub use model::{File, FileOrFolder, FileToDelete, FileTree, Folder};
pub use scan::{ScanItem, ScanOptions, scan_folder, scan_into_tree};
pub use tiles::{Area, Board, FileMetadata, FileType, RectFloat, Tile, TreeMap, files_in_folder};
