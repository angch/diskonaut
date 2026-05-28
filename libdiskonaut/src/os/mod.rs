mod unix;

pub use unix::{is_user_admin, size_on_disk_fast};

#[cfg(test)]
mod tests;
