mod model;
mod view;
mod worker;

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

use crate::model::{Effect, Key, Model, Msg};
use crate::worker::Request;

fn map_key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        _ => return None,
    })
}

fn run(
    terminal: &mut DefaultTerminal,
    from_worker: &Receiver<Msg>,
    to_worker: &async_channel::Sender<Request>,
) -> std::io::Result<()> {
    let mut model = Model::default();
    if let Ok(size) = terminal.size() {
        model.size = (size.width, size.height);
    }
    loop {
        terminal.draw(|frame| view::view(&model, frame))?;
        let mut msgs: Vec<Msg> = from_worker.try_iter().collect();
        // Wait briefly for terminal input so daemon updates still redraw promptly.
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if let Some(key) = map_key(key.code) {
                        msgs.push(Msg::Key(key));
                    }
                }
                Event::Resize(w, h) => msgs.push(Msg::Resize(w, h)),
                _ => {}
            }
        }
        for msg in msgs {
            for effect in model.update(msg) {
                match effect {
                    Effect::Quit => return Ok(()),
                    Effect::Refresh => {
                        let _ = to_worker.try_send(Request::Refresh);
                    }
                    Effect::Request(command) => {
                        let _ = to_worker.try_send(Request::Command(command));
                    }
                }
            }
        }
    }
}

fn main() -> std::io::Result<()> {
    let (to_ui, from_worker) = channel();
    let to_worker = worker::spawn(to_ui);
    // ratatui::init installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &from_worker, &to_worker);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests;
