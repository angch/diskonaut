use ::std::fs::File;
use ::std::io::Write;
use ::std::path::PathBuf;

use super::{ScanItem, ScanOptions, scan_folder, scan_into_tree};

fn temp_scan_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("diskonaut_scan_test_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn scan_folder_finds_files() {
    let dir = temp_scan_dir("finds_files");
    let file_path = dir.join("a.txt");
    let mut file = File::create(&file_path).expect("create file");
    file.write_all(b"hello").expect("write file");

    let options = ScanOptions {
        parallel: false,
        ..ScanOptions::default()
    };
    let entries: Vec<_> = scan_folder(&dir, options)
        .filter_map(|item| match item {
            ScanItem::Entry { path, .. } => Some(path),
            ScanItem::ReadError => None,
        })
        .collect();

    assert!(
        entries.iter().any(|p| p == &file_path),
        "expected walk to include {file_path:?}, got {entries:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scan_into_tree_aggregates_sizes() {
    let dir = temp_scan_dir("aggregates");
    let file_path = dir.join("data.bin");
    let mut file = File::create(&file_path).expect("create file");
    file.write_all(&[0u8; 1024]).expect("write file");

    let options = ScanOptions {
        parallel: false,
        show_apparent_size: true,
        ..ScanOptions::default()
    };
    let (tree, failed) = scan_into_tree(&dir, options);

    assert_eq!(failed, 0);
    assert!(tree.get_total_size() >= 1024);

    let _ = std::fs::remove_dir_all(&dir);
}
