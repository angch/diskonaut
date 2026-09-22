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
    // Canonicalized because the app always scans a canonical path: `Opts::resolve_folder`
    // resolves the folder before it reaches `FileTree`, which canonicalizes its own root too. On
    // macOS `temp_dir()` is `/var/...`, a symlink to `/private/var/...`, so an uncanonicalized
    // path here fails `strip_prefix` against that root and every entry is silently dropped.
    let dir = dir.canonicalize().expect("canonicalize temp dir");
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
    use ::std::path::{Path, PathBuf};

    use crate::model::HardLinks;
    use crate::scan::SharedBlocks;

    /// The old ledger, kept as the specification: compare paths component by component.
    fn reference_charge(
        seen: &mut Vec<(u64, u64, Vec<PathBuf>)>,
        inode: u64,
        size: u64,
        directory: &Path,
    ) -> Option<usize> {
        let shared = |left: &Path, right: &Path| {
            left.components()
                .zip(right.components())
                .take_while(|(l, r)| l == r)
                .count()
        };
        match seen.iter_mut().find(|(i, _, _)| *i == inode) {
            Some((_, seen_size, _)) if *seen_size != size => None,
            Some((_, _, dirs)) => {
                if dirs.iter().any(|d| d == directory) {
                    return Some(directory.components().count());
                }
                let deepest = dirs.iter().map(|d| shared(d, directory)).max().unwrap_or(0);
                dirs.push(directory.to_path_buf());
                Some(deepest)
            }
            None => {
                seen.push((inode, size, vec![directory.to_path_buf()]));
                None
            }
        }
    }

    /// The interned ledger must answer exactly as the component-wise one does, in any order.
    #[test]
    fn interned_ledger_matches_component_wise_reference() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move |bound: u64| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) % bound
        };
        let names = ["a", "b", "c"];
        let mut links = HardLinks::default();
        let mut reference = Vec::new();
        for _ in 0..20_000 {
            let depth = next(5) as usize;
            // Spelled the way a path might arrive, not only canonically: the old ledger compared
            // component-wise, and the new one must agree on `a/b/`, `a/./b` and `a//b` too.
            let mut dir = ::std::ffi::OsString::new();
            for level in 0..depth {
                if level > 0 {
                    dir.push(["/", "//", "/./"][next(3) as usize]);
                }
                dir.push(names[next(3) as usize]);
            }
            if depth > 0 && next(4) == 0 {
                dir.push("/");
            }
            let dir = PathBuf::from(dir);
            let inode = next(40);
            let size = 1024 * (1 + next(2));
            assert_eq!(
                links.charge(SharedBlocks::Inode(inode), size, &dir),
                reference_charge(&mut reference, inode, size, &dir),
                "inode {inode} size {size} in {}",
                dir.display()
            );
        }
        assert_eq!(links.tracked(), reference.len());
    }

    /// A reflinked file is the hard-link rule again: the same blocks reached twice inside one
    /// folder count once, while siblings each hold them in full.
    #[test]
    fn reflinked_blocks_are_charged_once_per_folder() {
        let mut links = HardLinks::default();
        let blocks = SharedBlocks::Extent(0x4000_0000);
        // First sighting charges the whole path.
        assert_eq!(links.charge(blocks, 1024, Path::new("a")), None);
        // A copy in the same folder is already paid for, down to that folder's depth.
        assert_eq!(links.charge(blocks, 1024, Path::new("a")), Some(1));
        // A copy in a sibling: the root above them has it, `b` itself does not.
        assert_eq!(links.charge(blocks, 1024, Path::new("b")), Some(0));
        assert_eq!(links.tracked_reflinks(), 1);
        assert_eq!(links.tracked(), 0, "no hard links were charged");
    }

    /// An inode number and a physical block offset are unrelated numbers. Sharing one map would
    /// silently merge a hard-linked file with a reflinked one whose extent happened to match.
    #[test]
    fn inode_and_extent_identities_do_not_collide() {
        let mut links = HardLinks::default();
        assert_eq!(
            links.charge(SharedBlocks::Inode(42), 1024, Path::new("a")),
            None
        );
        assert_eq!(
            links.charge(SharedBlocks::Extent(42), 1024, Path::new("a")),
            None,
            "the same number under a different identity is a different file"
        );
        assert_eq!(links.tracked(), 1);
        assert_eq!(links.tracked_reflinks(), 1);
    }

    /// Two files starting at one physical extent but differing in length share only part of
    /// themselves. Charging in full overstates; merging them would understate.
    #[test]
    fn partly_shared_files_are_charged_in_full() {
        let mut links = HardLinks::default();
        let blocks = SharedBlocks::Extent(0x8000);
        assert_eq!(links.charge(blocks, 4096, Path::new("a")), None);
        assert_eq!(links.charge(blocks, 8192, Path::new("b")), None);
    }

    /// `charge` returns the depth already accounted for, so folders below it still pay.
    #[test]
    fn spellings_of_one_directory_are_one_directory() {
        let mut links = HardLinks::default();
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 1024, Path::new("a/b")),
            None
        );
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 1024, Path::new("a/b/")),
            Some(2)
        );
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 1024, Path::new("a//b")),
            Some(2)
        );
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 1024, Path::new("a/./b")),
            Some(2)
        );
    }

    #[test]
    fn first_sighting_charges_the_whole_path() {
        let mut links = HardLinks::default();
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 1024, Path::new("a")),
            None
        );
        assert_eq!(links.tracked(), 1);
    }

    #[test]
    fn a_second_link_in_the_same_folder_is_already_paid_for() {
        let mut links = HardLinks::default();
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 1024, Path::new("a")),
            None
        );
        // "a" is at depth 1 and already holds it, so nothing below depth 1 owes anything.
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 1024, Path::new("a")),
            Some(1)
        );
    }

    #[test]
    fn a_link_in_a_sibling_folder_is_only_paid_for_by_shared_ancestors() {
        let mut links = HardLinks::default();
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 1024, Path::new("a")),
            None
        );
        // Only the root is shared, so "b" itself has not been charged yet.
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 1024, Path::new("b")),
            Some(0)
        );
    }

    /// The case a single running common-ancestor gets wrong: once two links have collapsed the
    /// common ancestor to the root, a third link beside one of them must still be recognised.
    #[test]
    fn a_third_link_beside_an_earlier_one_is_already_paid_for() {
        for order in [["b", "a", "a"], ["a", "b", "a"]] {
            let mut links = HardLinks::default();
            assert_eq!(
                links.charge(SharedBlocks::Inode(1), 1024, Path::new(order[0])),
                None
            );
            links.charge(SharedBlocks::Inode(1), 1024, Path::new(order[1]));
            let third = links.charge(SharedBlocks::Inode(1), 1024, Path::new(order[2]));
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
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 2048, Path::new("other")),
            None
        );
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 2048, Path::new("one/two")),
            Some(0)
        );
        // "one/two" holds it already, so nothing below depth 2 owes anything.
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 2048, Path::new("one/two")),
            Some(2)
        );
        // A sibling of "two" shares only "one".
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 2048, Path::new("one/three")),
            Some(1)
        );
    }

    /// Inode numbers repeat across the volumes of a macOS volume group, so a differing size means
    /// a different file and must not be deduplicated.
    #[test]
    fn a_reused_inode_number_with_a_different_size_is_a_different_file() {
        let mut links = HardLinks::default();
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 1024, Path::new("a")),
            None
        );
        assert_eq!(
            links.charge(SharedBlocks::Inode(1), 4096, Path::new("b")),
            None
        );
    }
}
