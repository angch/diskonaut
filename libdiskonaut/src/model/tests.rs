use ::std::ffi::OsString;
use ::std::fs;
use ::std::io::Write;
use ::std::path::PathBuf;

use crate::model::{FileOrFolder, FileTree, Folder};

#[test]
fn folder_add_file_updates_size() {
    let mut folder = Folder::from(OsString::from("root"));
    folder.add_file(PathBuf::from("a.txt"), 100);
    folder.add_file(PathBuf::from("b.txt"), 250);
    assert_eq!(folder.size, 350);
    assert_eq!(folder.num_descendants, 2);
}

#[test]
fn folder_nested_path() {
    let mut folder = Folder::from(OsString::from("root"));
    folder.add_file(PathBuf::from("sub/file.txt"), 42);
    assert_eq!(folder.size, 42);
    let sub = folder.path(vec!["sub".into()]).expect("subfolder exists");
    match sub {
        FileOrFolder::Folder(subfolder) => {
            assert_eq!(subfolder.size, 42);
        }
        FileOrFolder::File(_) => panic!("expected folder"),
    }
}

#[test]
fn file_tree_delete_path() {
    let dir = std::env::temp_dir().join("diskonaut_model_test_delete");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir");
    let file_path = dir.join("gone.txt");
    let mut f = fs::File::create(&file_path).expect("create");
    f.write_all(b"x").expect("write");

    let metadata = fs::metadata(&file_path).expect("metadata");
    let mut tree = FileTree::new(Folder::new(&dir), dir.clone());
    tree.add_entry(
        crate::EntryMeta {
            size: metadata.len(),
            links: 1,
            is_dir: false,
            ..crate::EntryMeta::default()
        },
        &file_path,
    );
    assert_eq!(tree.get_total_descendants(), 1);

    let to_delete = crate::FileToDelete {
        path_in_filesystem: dir.clone(),
        path_to_file: vec!["gone.txt".into()],
        file_type: crate::tiles::FileType::File,
        num_descendants: None,
        size: metadata.len().into(),
    };
    tree.delete_file(&to_delete);
    assert_eq!(tree.get_total_descendants(), 0);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn deleting_a_folder_removes_the_folder_itself_from_the_count() {
    // Built the way a scan builds it: directories arrive as entries in their own right.
    let mut root = Folder::from(OsString::from("root"));
    root.add_file(PathBuf::from("keep.txt"), 10);
    root.add_folder(PathBuf::from("sub"));
    root.add_file(PathBuf::from("sub/one.txt"), 20);
    root.add_file(PathBuf::from("sub/two.txt"), 30);
    root.add_folder(PathBuf::from("sub/deeper"));
    root.add_file(PathBuf::from("sub/deeper/three.txt"), 40);
    // keep.txt, sub, sub/one.txt, sub/two.txt, sub/deeper, sub/deeper/three.txt
    assert_eq!(root.num_descendants, 6);
    assert_eq!(root.size, 100);

    root.delete_path(&[OsString::from("sub")]);

    // Only keep.txt is left, and "sub" itself is gone along with its four descendants.
    assert_eq!(root.size, 10);
    assert_eq!(
        root.num_descendants, 1,
        "the deleted folder itself must be subtracted too, not just its contents"
    );
}

#[test]
fn deleting_a_nested_folder_updates_every_ancestor() {
    let mut root = Folder::from(OsString::from("root"));
    root.add_folder(PathBuf::from("a"));
    root.add_folder(PathBuf::from("a/b"));
    root.add_folder(PathBuf::from("a/b/c"));
    root.add_file(PathBuf::from("a/b/c/file.txt"), 64);
    root.add_file(PathBuf::from("a/other.txt"), 8);
    // a, a/b, a/b/c, a/b/c/file.txt, a/other.txt
    assert_eq!(root.num_descendants, 5);

    root.delete_path(&[OsString::from("a"), OsString::from("b")]);

    // a and a/other.txt remain; b, b/c and b/c/file.txt are gone.
    assert_eq!(root.num_descendants, 2);
    assert_eq!(root.size, 8);
    let a = root.path(vec!["a".into()]).expect("a still exists");
    match a {
        FileOrFolder::Folder(a) => {
            assert_eq!(a.num_descendants, 1);
            assert_eq!(a.size, 8);
        }
        FileOrFolder::File(_) => panic!("expected folder"),
    }
}

mod hard_links {
    use ::std::path::Path;

    use crate::model::HardLinks;

    /// `charge` returns the depth already accounted for, so folders below it still pay.
    #[test]
    fn first_sighting_charges_the_whole_path() {
        let mut links = HardLinks::default();
        assert_eq!(links.charge(1, 1024, Path::new("a")), None);
        assert_eq!(links.tracked(), 1);
    }

    #[test]
    fn a_second_link_in_the_same_folder_is_already_paid_for() {
        let mut links = HardLinks::default();
        assert_eq!(links.charge(1, 1024, Path::new("a")), None);
        // "a" is at depth 1 and already holds it, so nothing below depth 1 owes anything.
        assert_eq!(links.charge(1, 1024, Path::new("a")), Some(1));
    }

    #[test]
    fn a_link_in_a_sibling_folder_is_only_paid_for_by_shared_ancestors() {
        let mut links = HardLinks::default();
        assert_eq!(links.charge(1, 1024, Path::new("a")), None);
        // Only the root is shared, so "b" itself has not been charged yet.
        assert_eq!(links.charge(1, 1024, Path::new("b")), Some(0));
    }

    /// The case a single running common-ancestor gets wrong: once two links have collapsed the
    /// common ancestor to the root, a third link beside one of them must still be recognised.
    #[test]
    fn a_third_link_beside_an_earlier_one_is_already_paid_for() {
        for order in [["b", "a", "a"], ["a", "b", "a"]] {
            let mut links = HardLinks::default();
            assert_eq!(links.charge(1, 1024, Path::new(order[0])), None);
            links.charge(1, 1024, Path::new(order[1]));
            let third = links.charge(1, 1024, Path::new(order[2]));
            assert_eq!(
                third,
                Some(1),
                "with order {order:?} the folder {:?} already holds this file",
                order[2]
            );
        }
    }

    #[test]
    fn deeper_folders_are_recognised_independently_of_shallower_ones() {
        let mut links = HardLinks::default();
        assert_eq!(links.charge(1, 2048, Path::new("other")), None);
        assert_eq!(links.charge(1, 2048, Path::new("one/two")), Some(0));
        // "one/two" holds it already, so nothing below depth 2 owes anything.
        assert_eq!(links.charge(1, 2048, Path::new("one/two")), Some(2));
        // A sibling of "two" shares only "one".
        assert_eq!(links.charge(1, 2048, Path::new("one/three")), Some(1));
    }

    /// Inode numbers repeat across the volumes of a macOS volume group, so a differing size means
    /// a different file and must not be deduplicated.
    #[test]
    fn a_reused_inode_number_with_a_different_size_is_a_different_file() {
        let mut links = HardLinks::default();
        assert_eq!(links.charge(1, 1024, Path::new("a")), None);
        assert_eq!(links.charge(1, 4096, Path::new("b")), None);
    }
}
