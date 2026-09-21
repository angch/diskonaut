use ::std::fs::File;
use ::std::io::Write;
use ::std::path::PathBuf;

use super::{EntryMeta, NamedEntry, ScanItem, ScanOptions, scan_directories, scan_folder, scan_into_tree};

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
        show_apparent_size: false,
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

#[test]
fn scan_directories_groups_entries_by_directory() {
    let dir = temp_scan_dir("grouped");
    let sub = dir.join("sub");
    std::fs::create_dir(&sub).expect("create subdir");
    File::create(dir.join("top.txt"))
        .expect("create file")
        .write_all(b"top")
        .expect("write file");
    File::create(sub.join("inner.txt"))
        .expect("create file")
        .write_all(b"inner")
        .expect("write file");

    let options = ScanOptions {
        parallel: false,
        show_apparent_size: true,
        ..ScanOptions::default()
    };
    let mut named: Vec<_> = scan_directories(&dir, options)
        .flat_map(|directory| {
            let parent = directory.path.to_path_buf();
            directory
                .entries
                .into_iter()
                .map(move |entry| parent.join(&entry.name))
        })
        .collect();
    named.sort();

    assert_eq!(named, vec![sub.clone(), dir.join("top.txt"), sub.join("inner.txt")].into_iter().collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scan_into_tree_counts_nested_entries() {
    let dir = temp_scan_dir("nested");
    let sub = dir.join("a").join("b");
    std::fs::create_dir_all(&sub).expect("create nested dirs");
    File::create(sub.join("deep.bin"))
        .expect("create file")
        .write_all(&[7u8; 2048])
        .expect("write file");

    let options = ScanOptions {
        show_apparent_size: true,
        ..ScanOptions::default()
    };
    let (tree, failed) = scan_into_tree(&dir, options);

    assert_eq!(failed, 0);
    assert_eq!(tree.get_total_size(), 2048);
    // "a", "a/b" and "a/b/deep.bin"
    assert_eq!(tree.get_total_descendants(), 3);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Build a small fixture tree and return the full set of paths it should produce.
fn fixture_tree(name: &str) -> (PathBuf, std::collections::BTreeSet<PathBuf>) {
    let dir = temp_scan_dir(name);
    let a = dir.join("a");
    let b = dir.join("b");
    let nested = a.join("nested");
    std::fs::create_dir_all(&nested).expect("mkdir a/nested");
    std::fs::create_dir_all(&b).expect("mkdir b");
    let mut expected = std::collections::BTreeSet::new();
    expected.insert(a.clone());
    expected.insert(b.clone());
    expected.insert(nested.clone());
    for (parent, file) in [
        (&dir, "top.txt"),
        (&a, "one.txt"),
        (&a, "two.txt"),
        (&nested, "deep.txt"),
        (&b, "three.txt"),
    ] {
        let path = parent.join(file);
        File::create(&path)
            .expect("create file")
            .write_all(b"0123456789")
            .expect("write file");
        expected.insert(path);
    }
    (dir, expected)
}

fn collect_paths(dir: &std::path::Path, options: ScanOptions) -> std::collections::BTreeSet<PathBuf> {
    scan_directories(dir, options)
        .flat_map(|directory| {
            let parent = directory.path.to_path_buf();
            directory
                .entries
                .into_iter()
                .map(move |entry| parent.join(&entry.name))
        })
        .collect()
}

#[test]
fn scan_directories_reports_every_entry_exactly_once() {
    let (dir, expected) = fixture_tree("every_entry");
    let mut seen = Vec::new();
    for directory in scan_directories(&dir, ScanOptions::default()) {
        let parent = directory.path.to_path_buf();
        seen.extend(directory.entries.iter().map(|entry| parent.join(&entry.name)));
    }
    let unique: std::collections::BTreeSet<_> = seen.iter().cloned().collect();
    assert_eq!(unique, expected, "wrong set of entries");
    assert_eq!(seen.len(), unique.len(), "an entry was reported twice: {seen:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The `dua-core` grouping is only reached off macOS, so exercise it directly everywhere.
#[test]
fn fallback_grouping_reports_every_entry_exactly_once() {
    let (dir, expected) = fixture_tree("fallback_every_entry");
    let mut seen = Vec::new();
    for directory in super::fallback::group_by_directory(&dir, ScanOptions::default()) {
        let parent = directory.path.to_path_buf();
        seen.extend(directory.entries.iter().map(|entry| parent.join(&entry.name)));
    }
    let unique: std::collections::BTreeSet<_> = seen.iter().cloned().collect();
    assert_eq!(unique, expected, "wrong set of entries");
    assert_eq!(seen.len(), unique.len(), "an entry was reported twice: {seen:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Both walkers must agree, so that the benchmark compares like with like.
#[test]
fn both_walkers_agree_on_the_same_tree() {
    let (dir, expected) = fixture_tree("walkers_agree");
    let options = ScanOptions {
        show_apparent_size: true,
        ..ScanOptions::default()
    };

    let native = collect_paths(&dir, options);
    let fallback: std::collections::BTreeSet<PathBuf> =
        super::fallback::group_by_directory(&dir, options)
            .flat_map(|directory| {
                let parent = directory.path.to_path_buf();
                directory
                    .entries
                    .into_iter()
                    .map(move |entry| parent.join(&entry.name))
            })
            .collect();

    assert_eq!(native, expected);
    assert_eq!(fallback, expected);

    let (tree, failed) = scan_into_tree(&dir, options);
    assert_eq!(failed, 0);
    assert_eq!(tree.get_total_size(), 50, "five ten-byte files");
    assert_eq!(tree.get_total_descendants(), expected.len() as u64);

    let _ = std::fs::remove_dir_all(&dir);
}

/// A scan must never attribute anything to a directory outside the tree it was asked about.
#[test]
fn entries_outside_the_scan_root_are_ignored() {
    let dir = temp_scan_dir("outside_root");
    let mut tree = crate::FileTree::new(crate::Folder::new(&dir), dir.clone());
    tree.add_dir_entries(
        std::path::Path::new("/somewhere/else"),
        &[NamedEntry {
            name: "intruder".into(),
            meta: EntryMeta {
                size: 4096,
                is_dir: false,
            },
        }],
    );
    assert_eq!(tree.get_total_size(), 0);
    assert_eq!(tree.get_total_descendants(), 0);

    let _ = std::fs::remove_dir_all(&dir);
}
