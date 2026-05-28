use ::std::fs::File;
use ::std::io::Write;

#[test]
fn is_user_admin_returns_bool() {
    let _ = crate::os::is_user_admin();
}

#[test]
fn size_on_disk_fast_is_at_least_file_length() {
    let dir = std::env::temp_dir().join("diskonaut_os_block_size_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");

    let file_path = dir.join("data.bin");
    let mut file = File::create(&file_path).expect("create file");
    file.write_all(&[0u8; 1024]).expect("write file");

    let metadata = std::fs::metadata(&file_path).expect("stat file");
    let on_disk = crate::os::size_on_disk_fast(&metadata);

    assert!(on_disk >= 1024, "on-disk size {on_disk} should cover 1024 logical bytes");

    let _ = std::fs::remove_dir_all(&dir);
}
