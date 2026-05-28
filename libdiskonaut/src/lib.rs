//! Core disk usage model, treemap layout, and helpers (no TUI).

pub mod format;
pub mod model;
pub mod os;
pub mod tiles;

pub use model::{File, FileOrFolder, FileToDelete, FileTree, Folder};
