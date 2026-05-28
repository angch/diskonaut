use ::ratatui::backend::{Backend, ClearType, WindowSize};
use ::ratatui::buffer::Cell;
use ::ratatui::layout::{Position, Size};
use ::std::collections::HashMap;
use ::std::io;
use ::std::sync::{Arc, Mutex};

#[derive(Hash, Debug, PartialEq)]
pub enum TerminalEvent {
    Clear,
    HideCursor,
    ShowCursor,
    GetCursor,
    Flush,
    Draw,
}

pub struct TestBackend {
    pub events: Arc<Mutex<Vec<TerminalEvent>>>,
    pub draw_events: Arc<Mutex<Vec<String>>>,
    terminal_width: Arc<Mutex<u16>>,
    terminal_height: Arc<Mutex<u16>>,
}

impl TestBackend {
    pub fn new(
        log: Arc<Mutex<Vec<TerminalEvent>>>,
        draw_log: Arc<Mutex<Vec<String>>>,
        terminal_width: Arc<Mutex<u16>>,
        terminal_height: Arc<Mutex<u16>>,
    ) -> TestBackend {
        TestBackend {
            events: log,
            draw_events: draw_log,
            terminal_width,
            terminal_height,
        }
    }

    fn terminal_size(&self) -> Size {
        let terminal_height = self.terminal_height.lock().unwrap();
        let terminal_width = self.terminal_width.lock().unwrap();
        Size {
            width: *terminal_width,
            height: *terminal_height,
        }
    }
}

#[derive(Hash, Eq, PartialEq)]
struct Point {
    x: u16,
    y: u16,
}

impl Backend for TestBackend {
    type Error = io::Error;

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.events.lock().unwrap().push(TerminalEvent::Clear);
        Ok(())
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        if clear_type == ClearType::All {
            self.clear()?;
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.events.lock().unwrap().push(TerminalEvent::HideCursor);
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.events.lock().unwrap().push(TerminalEvent::ShowCursor);
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.events.lock().unwrap().push(TerminalEvent::GetCursor);
        Ok(Position { x: 0, y: 0 })
    }

    fn set_cursor_position<P>(&mut self, _position: P) -> Result<(), Self::Error>
    where
        P: Into<Position>,
    {
        Ok(())
    }

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.events.lock().unwrap().push(TerminalEvent::Draw);
        let mut string = String::with_capacity(content.size_hint().0 * 3);
        let mut coordinates = HashMap::new();
        for (x, y, cell) in content {
            coordinates.insert(Point { x, y }, cell);
        }
        let Size {
            width: terminal_width,
            height: terminal_height,
        } = self.terminal_size();
        for y in 0..terminal_height {
            for x in 0..terminal_width {
                match coordinates.get(&Point { x, y }) {
                    Some(cell) => {
                        // this will contain no style information at all
                        // should be good enough for testing
                        string.push_str(cell.symbol());
                    }
                    None => {
                        string.push(' ');
                    }
                }
            }
            string.push('\n');
        }
        self.draw_events.lock().unwrap().push(string);
        Ok(())
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(self.terminal_size())
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        let columns_rows = self.terminal_size();
        Ok(WindowSize {
            columns_rows,
            pixels: Size {
                width: 0,
                height: 0,
            },
        })
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.events.lock().unwrap().push(TerminalEvent::Flush);
        Ok(())
    }
}
