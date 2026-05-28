#[cfg(not(target_os = "windows"))]
#[test]
fn is_user_admin_returns_bool_on_unix() {
    let _ = crate::os::unix::is_user_admin();
}

#[cfg(target_os = "windows")]
#[test]
fn is_user_admin_returns_bool_on_windows() {
    let _ = crate::os::windows::is_user_admin();
}
