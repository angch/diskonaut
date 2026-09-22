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

/// One entry holds nearly everything; the rest would round to zero cells. The board must still
/// mark that they exist, or the view looks as though the scan missed them.
#[test]
fn tiny_siblings_of_a_huge_entry_still_get_a_small_files_marker() {
    let mut root = Folder::new(Path::new("/tmp/example"));
    root.add_file(std::path::PathBuf::from("target"), 2_000_000_000);
    for name in ["a", "b", "c", "d", "e"] {
        root.add_file(std::path::PathBuf::from(name), 1_000_000);
    }

    let area = Area {
        x: 1,
        y: 1,
        width: 158,
        height: 36,
    };
    let mut board = Board::new(&root);
    board.change_area(&area);
    board.change_files(&root);

    assert_eq!(board.tiles.len(), 1, "only the huge entry is drawable");
    let (x, y) = board
        .unrenderable_tile_coordinates
        .expect("hidden entries must leave a small-files marker");
    let right = area.x + area.width;
    let bottom = area.y + area.height;
    assert!(x >= area.x && y >= area.y, "marker starts inside the board");
    assert!(
        right - x >= 4 && bottom - y >= 3,
        "marker is wide enough to draw: ({x}, {y}) in {area:?}"
    );
}

#[test]
fn small_files_marker_is_absent_when_everything_fits() {
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

    assert!(board.unrenderable_tile_coordinates.is_none());
}
