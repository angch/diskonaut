use ::std::fs::Metadata;
use ::std::os::unix::fs::MetadataExt;

use rustix::process::{Uid, geteuid};

pub fn is_user_admin() -> bool {
    geteuid() == Uid::ROOT
}

/// Allocated size on disk from directory-walk metadata (`st_blocks` × 512-byte units).
pub fn size_on_disk_fast(metadata: &Metadata) -> u64 {
    metadata.blocks().saturating_mul(512)
}

pub fn volume_id(path: &::std::path::Path) -> Option<u64> {
    ::std::fs::metadata(path).map(|m| m.dev()).ok()
}

pub fn link_count(path: &::std::path::Path) -> u64 {
    ::std::fs::metadata(path).map(|m| m.nlink()).unwrap_or(1)
}
