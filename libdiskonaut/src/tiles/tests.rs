use ::std::ffi::OsString;
use ::std::path::Path;

use crate::model::Folder;
use crate::tiles::{Area, Board, FileType, files_in_folder};

#[test]
fn board_produces_tiles_for_folder() {
    let mut root = Folder::new(Path::new("/tmp/example"));
    root.add_file(std::path::PathBuf::from("a"), 600);
    root.add_file(std::path::PathBuf::from("b"), 400);

    let mut board = Board::new(&root);
    board.change_area(&Area {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    });
    board.change_files(&root);

    assert_eq!(board.tiles.len(), 2);
    assert!(
        board
            .tiles
            .iter()
            .any(|t| t.file_type == FileType::File && t.size == 600)
    );
}

#[test]
fn files_in_folder_percentages_sum_to_one() {
    let mut root = Folder::from(OsString::from("root"));
    root.add_file("x".into(), 75);
    root.add_file("y".into(), 25);

    let files = files_in_folder(&root, 0);
    assert_eq!(files.len(), 2);
    let sum: f64 = files.iter().map(|f| f.percentage).sum();
    assert!((sum - 1.0).abs() < f64::EPSILON);
}
