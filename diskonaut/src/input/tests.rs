use ::std::path::PathBuf;
use ::std::sync::mpsc;

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use super::controls::{
    handle_keypress_delete_file_mode, handle_keypress_exiting_mode, handle_keypress_loading_mode,
    handle_keypress_normal_mode, handle_keypress_screen_too_small,
};
use crate::app::{App, UiMode};
use crate::config::Keybinds;

fn key_char(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

fn test_app(width: u16, height: u16) -> App<TestBackend> {
    let dir = std::env::temp_dir().join("diskonaut_input_test");
    let _ = std::fs::create_dir_all(&dir);
    let (tx, _rx) = mpsc::sync_channel(1);
    App::new(
        TestBackend::new(width, height),
        dir,
        tx,
        true,
        Keybinds::default(),
    )
}

#[test]
fn loading_mode_q_prompts_exit() {
    let mut app = test_app(80, 24);
    app.ui_mode = UiMode::Loading;
    handle_keypress_loading_mode(key_char('q'), &mut app);
    assert!(matches!(app.ui_mode, UiMode::Exiting { .. }));
}

#[test]
fn normal_mode_d_opens_delete_flow() {
    let mut app = test_app(80, 24);
    app.ui_mode = UiMode::Normal;
    app.loaded = true;
    handle_keypress_normal_mode(key_char('d'), &mut app);
    // Without a selected tile, mode stays normal.
    assert!(matches!(app.ui_mode, UiMode::Normal));
}

#[test]
fn screen_too_small_ctrl_c_exits() {
    let mut app = test_app(80, 24);
    app.ui_mode = UiMode::ScreenTooSmall;
    handle_keypress_screen_too_small(
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        &mut app,
    );
    assert!(!app.is_running);
}

#[test]
fn exiting_mode_y_quits() {
    let mut app = test_app(80, 24);
    app.ui_mode = UiMode::Exiting { app_loaded: true };
    handle_keypress_exiting_mode(key_char('y'), &mut app);
    assert!(!app.is_running);
}

#[test]
fn delete_mode_n_returns_to_normal() {
    let mut app = test_app(80, 24);
    let file = libdiskonaut::FileToDelete {
        path_in_filesystem: PathBuf::from("/tmp"),
        path_to_file: vec!["file".into()],
        file_type: libdiskonaut::tiles::FileType::File,
        num_descendants: None,
        size: 1,
    };
    app.ui_mode = UiMode::DeleteFile(file.clone());
    handle_keypress_delete_file_mode(key_char('n'), &mut app, file);
    assert!(matches!(app.ui_mode, UiMode::Normal));
}
