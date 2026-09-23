#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{is_user_admin, link_count, size_on_disk_fast, volume_id};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{is_user_admin, link_count, size_on_disk_fast, volume_id};

#[cfg(test)]
mod tests;
