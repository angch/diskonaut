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
