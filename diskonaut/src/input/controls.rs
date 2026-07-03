use ::ratatui::backend::Backend;
use ratatui::crossterm::event::Event;
use ratatui::crossterm::event::read;

use crate::App;
use crate::config::Keybinds;
use libdiskonaut::FileToDelete;

#[derive(Clone)]
pub struct TerminalEvents;

impl Iterator for TerminalEvents {
    type Item = Event;
    fn next(&mut self) -> Option<Event> {
        Some(read().unwrap())
    }
}

pub fn handle_keypress_loading_mode<B: Backend>(evt: Event, app: &mut App<B>) {
    let kb = &app.keybinds;
    if kb.is_quit(&evt) {
        app.prompt_exit();
    } else if kb.delete.matches_event(&evt) {
        app.show_warning_modal();
    } else if kb.is_move_right(&evt) {
        app.move_selected_right();
    } else if kb.is_move_left(&evt) {
        app.move_selected_left();
    } else if kb.is_move_down(&evt) {
        app.move_selected_down();
    } else if kb.is_move_up(&evt) {
        app.move_selected_up();
    } else if kb.is_zoom_in(&evt) {
        app.zoom_in();
    } else if kb.zoom_out.matches_event(&evt) {
        app.zoom_out();
    } else if kb.reset_zoom.matches_event(&evt) {
        app.reset_zoom();
    } else if kb.is_enter(&evt) {
        app.handle_enter();
    } else if kb.parent.matches_event(&evt) {
        app.go_up();
    }
}

pub fn handle_keypress_normal_mode<B: Backend>(evt: Event, app: &mut App<B>) {
    let kb = &app.keybinds;
    if kb.is_quit(&evt) {
        app.prompt_exit();
    } else if kb.delete.matches_event(&evt) {
        app.prompt_file_deletion();
    } else if kb.is_move_right(&evt) {
        app.move_selected_right();
    } else if kb.is_move_left(&evt) {
        app.move_selected_left();
    } else if kb.is_move_down(&evt) {
        app.move_selected_down();
    } else if kb.is_move_up(&evt) {
        app.move_selected_up();
    } else if kb.is_zoom_in(&evt) {
        app.zoom_in();
    } else if kb.zoom_out.matches_event(&evt) {
        app.zoom_out();
    } else if kb.reset_zoom.matches_event(&evt) {
        app.reset_zoom();
    } else if kb.is_enter(&evt) {
        app.handle_enter();
    } else if kb.parent.matches_event(&evt) {
        app.go_up();
    }
}

pub fn handle_keypress_delete_file_mode<B: Backend>(
    evt: Event,
    app: &mut App<B>,
    file_to_delete: FileToDelete,
) {
    let kb = &app.keybinds;
    if kb.is_quit(&evt) || kb.is_cancel(&evt) {
        app.normal_mode();
    } else if kb.is_confirm(&evt) {
        app.delete_file(&file_to_delete);
    }
}

pub fn handle_keypress_error_message<B: Backend>(evt: Event, app: &mut App<B>) {
    let kb = &app.keybinds;
    if kb.is_quit(&evt) || kb.parent.matches_event(&evt) {
        app.normal_mode();
    }
}

pub fn handle_keypress_screen_too_small<B: Backend>(evt: Event, app: &mut App<B>) {
    if app.keybinds.is_quit(&evt) {
        app.exit();
    }
}

pub fn handle_keypress_exiting_mode<B: Backend>(evt: Event, app: &mut App<B>) {
    let kb = &app.keybinds;
    if kb.is_quit(&evt) || kb.is_cancel(&evt) {
        app.reset_ui_mode();
        // we have to manually call render here to make sure ui gets updated
        // because reset_ui_mode does not call it itself
        app.render();
    } else if kb.is_confirm(&evt) {
        app.exit();
    }
}

pub fn handle_keypress_warning_message<B: Backend>(_evt: Event, app: &mut App<B>) {
    app.reset_ui_mode();
}

/// Returns true when the stdin thread should pause briefly after handling (quit / confirm keys).
pub fn needs_quit_delay(evt: &Event, keybinds: &Keybinds) -> bool {
    keybinds.is_quit(evt) || keybinds.is_confirm(evt)
}
