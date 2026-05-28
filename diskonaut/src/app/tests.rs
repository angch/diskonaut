use ::std::ffi::{OsStr, OsString};
use ::std::fs::{self, File};
use ::std::io::Write;
use ::std::path::PathBuf;
use ::std::sync::mpsc;

use libdiskonaut::{ScanItem, ScanOptions, scan_folder};
use ratatui::backend::TestBackend;

use super::{App, UiMode};

fn temp_app_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("diskonaut_app_test_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn app_with_scanned_dir(dir: &PathBuf, width: u16, height: u16) -> App<TestBackend> {
    let (tx, _rx) = mpsc::sync_channel(1);
    let mut app = App::new(TestBackend::new(width, height), dir.clone(), tx, true);
    let options = ScanOptions {
        parallel: false,
        show_apparent_size: true,
        skip_hidden: false,
        follow_links: false,
    };
    for item in scan_folder(dir, options) {
        if let ScanItem::Entry {
            metadata,
            path: entry_path,
        } = item
        {
            app.add_entry_to_base_folder(&metadata, entry_path);
        }
    }
    app.start_ui();
    app
}

#[test]
fn render_marks_screen_too_small_below_minimum_size() {
    let dir = temp_app_dir("too_small");
    let mut app = app_with_scanned_dir(&dir, 40, 10);
    app.render();
    assert!(matches!(app.ui_mode, UiMode::ScreenTooSmall));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn enter_selected_enters_subfolder() {
    let dir = temp_app_dir("enter_subfolder");
    let sub = dir.join("sub");
    fs::create_dir(&sub).expect("create subfolder");
    let mut big_in_sub = File::create(sub.join("large.dat")).expect("create file in sub");
    big_in_sub
        .write_all(&vec![b'x'; 8192])
        .expect("write subfolder data");
    File::create(dir.join("tiny.txt"))
        .expect("create small root file")
        .write_all(b"x")
        .expect("write root file");

    let mut app = app_with_scanned_dir(&dir, 80, 24);
    assert!(
        app.file_tree
            .item_in_current_folder(OsStr::new("sub"))
            .is_some(),
        "scan should register subfolder in tree"
    );
    app.board.move_to_largest_folder();
    let selected = app
        .board
        .currently_selected()
        .expect("a folder tile should be selected");
    assert_eq!(selected.name, OsStr::new("sub"));
    assert_eq!(selected.file_type, libdiskonaut::tiles::FileType::Folder);
    app.handle_enter();

    assert_eq!(app.file_tree.get_current_path(), sub);
    assert_eq!(
        app.file_tree.current_folder_names,
        vec![OsString::from("sub")]
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn prompt_file_deletion_shows_confirmation() {
    let dir = temp_app_dir("delete_confirm");
    let target = dir.join("remove_me.txt");
    File::create(&target).expect("create file");

    let (tx, _rx) = mpsc::sync_channel(1);
    let mut app = App::new(TestBackend::new(80, 24), dir.clone(), tx, true);
    let meta = fs::metadata(&target).expect("metadata");
    app.add_entry_to_base_folder(&meta, target.clone());
    app.start_ui();
    let file_index = app
        .board
        .tiles
        .iter()
        .position(|t| t.name == OsStr::new("remove_me.txt"))
        .expect("file tile");
    app.board.set_selected_index(&file_index);
    app.prompt_file_deletion();

    assert!(target.exists(), "file should remain until user confirms");
    assert!(matches!(app.ui_mode, UiMode::DeleteFile(_)));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reset_ui_mode_from_error_returns_to_normal() {
    let dir = temp_app_dir("reset_mode");
    let mut app = app_with_scanned_dir(&dir, 80, 24);
    app.ui_mode = UiMode::ErrorMessage("oops".into());
    app.reset_ui_mode();
    assert!(matches!(app.ui_mode, UiMode::Normal));
    let _ = fs::remove_dir_all(&dir);
}
