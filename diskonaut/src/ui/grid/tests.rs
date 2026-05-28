use ::std::ffi::OsString;

use libdiskonaut::tiles::{FileType, Tile};
use ratatui::style::Color;

use super::tile_style;

fn sample_tile(file_type: FileType, width: u16) -> Tile {
    Tile {
        x: 0,
        y: 0,
        width,
        height: 10,
        name: OsString::from(if matches!(file_type, FileType::Folder) {
            "docs"
        } else {
            "readme.txt"
        }),
        size: 2048,
        descendants: if matches!(file_type, FileType::Folder) {
            Some(3)
        } else {
            None
        },
        percentage: 0.5,
        file_type,
    }
}

#[test]
fn selected_file_uses_magenta_on_gray() {
    let tile = sample_tile(FileType::File, 20);
    let (_, first, _) = tile_style(&tile, true);
    assert_eq!(first.fg, Some(Color::Magenta));
    assert_eq!(first.bg, Some(Color::Gray));
}

#[test]
fn selected_folder_uses_white_on_blue() {
    let tile = sample_tile(FileType::Folder, 30);
    let (_, first, _) = tile_style(&tile, true);
    assert_eq!(first.fg, Some(Color::White));
    assert_eq!(first.bg, Some(Color::Blue));
}

#[test]
fn unselected_folder_name_is_blue_bold() {
    let tile = sample_tile(FileType::Folder, 30);
    let (_, first, _) = tile_style(&tile, false);
    assert_eq!(first.fg, Some(Color::Blue));
}
