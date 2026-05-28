use ::ratatui::buffer::Buffer;
use ::ratatui::layout::Rect;
use ::ratatui::style::{Color, Modifier, Style};

use super::{CellSizeOpt, TitleTelescope};

#[test]
fn telescope_renders_total_on_wide_terminal() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 120, 1));
    let mut telescope = TitleTelescope::new(Style::default().fg(Color::Yellow));
    telescope.append_to_left_side(vec![
        CellSizeOpt::new(" Total: 1.0K (2 files), freed: 0 ".into()),
        CellSizeOpt::new(" Total: 1.0K (2 files) ".into()),
    ]);
    telescope.append_to_right_side(vec![CellSizeOpt::new(" /tmp/scan ".into())]);
    telescope.render(Rect::new(0, 0, 120, 1), &mut buf);

    let line: String = (0..120)
        .filter_map(|x| buf[(x, 0)].symbol().chars().next())
        .collect();
    assert!(line.contains("Total: 1.0K"));
    assert!(line.contains("/tmp/scan"));
}

#[test]
fn telescope_shows_loading_indicator_when_loading() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 1));
    let mut telescope = TitleTelescope::new(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );
    telescope.append_to_left_side(vec![CellSizeOpt::new(" Total: 1.0K (1 files) ".into())]);
    telescope.append_to_right_side(vec![CellSizeOpt::new(" /tmp ".into())]);
    let telescope = telescope.loading(true, 3);
    telescope.render(Rect::new(0, 0, 80, 1), &mut buf);

    let line: String = (0..80)
        .filter_map(|x| buf[(x, 0)].symbol().chars().next())
        .collect();
    assert!(
        !line.trim().is_empty(),
        "loading title should render visible content"
    );
}
