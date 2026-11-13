//! TUI state and drawing, kept free of I/O so every key, signal and size is testable with
//! Ratatui's `TestBackend`. Methods return the views to (re)load; the caller sends those
//! requests to the loader thread.
use std::collections::{BTreeMap, BTreeSet};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Tabs, Wrap};

use crate::Screen;
use crate::watch::Signal;

/// Below this size the layout cannot fit, so only a short notice is drawn.
const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 10;

pub(crate) struct Loaded {
    pub(crate) screen: Screen,
    pub(crate) body: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Link {
    #[default]
    Unknown,
    Connected,
    Disconnected,
}

pub(crate) struct App {
    screen: Screen,
    bodies: BTreeMap<Screen, String>,
    /// Views with a load in flight, so a burst of signals sends one request per view.
    pending: BTreeSet<Screen>,
    link: Link,
    help: bool,
    scroll: u16,
    quit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            screen: Screen::Overview,
            bodies: BTreeMap::new(),
            pending: BTreeSet::new(),
            link: Link::Unknown,
            help: false,
            scroll: 0,
            quit: false,
        }
    }
}

impl App {
    pub(crate) fn should_quit(&self) -> bool {
        self.quit
    }

    pub(crate) fn start(&mut self) -> Vec<Screen> {
        self.request(self.screen)
    }

    fn request(&mut self, screen: Screen) -> Vec<Screen> {
        if self.pending.insert(screen) {
            vec![screen]
        } else {
            Vec::new()
        }
    }

    fn select(&mut self, screen: Screen) -> Vec<Screen> {
        if screen != self.screen {
            self.screen = screen;
            self.scroll = 0;
        }
        // Every visit re-reads the daemon, as the shell always has.
        self.request(screen)
    }

    fn step(&mut self, forward: bool) -> Vec<Screen> {
        let index = Screen::ALL
            .iter()
            .position(|screen| *screen == self.screen)
            .unwrap_or(0);
        let count = Screen::ALL.len();
        let next = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        self.select(Screen::ALL[next])
    }

    fn body_lines(&self) -> u16 {
        self.bodies.get(&self.screen).map_or(0, |body| {
            u16::try_from(body.lines().count()).unwrap_or(u16::MAX)
        })
    }

    pub(crate) fn on_key(&mut self, key: KeyEvent) -> Vec<Screen> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return Vec::new();
        }
        // Esc is always "back": it closes help, and does nothing else.
        if self.help {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('h' | '?') | KeyCode::Enter
            ) {
                self.help = false;
            } else if key.code == KeyCode::Char('q') {
                self.quit = true;
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Char('q') => {
                self.quit = true;
                Vec::new()
            }
            KeyCode::Char('1' | 'o') => self.select(Screen::Overview),
            KeyCode::Char('2' | 'n') => self.select(Screen::Networks),
            KeyCode::Char('3' | 'd') => self.select(Screen::Devices),
            KeyCode::Char('4' | 'i') => self.select(Screen::Diagnostics),
            KeyCode::Tab | KeyCode::Right => self.step(true),
            KeyCode::BackTab | KeyCode::Left => self.step(false),
            KeyCode::Char('r') => self.request(self.screen),
            KeyCode::Char('h' | '?') => {
                self.help = true;
                Vec::new()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1).min(self.body_lines());
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                Vec::new()
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10).min(self.body_lines());
                Vec::new()
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                Vec::new()
            }
            KeyCode::Home => {
                self.scroll = 0;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn on_signal(&mut self, signal: Signal) -> Vec<Screen> {
        self.link = match signal {
            Signal::Connected | Signal::Changed => Link::Connected,
            Signal::Disconnected => Link::Disconnected,
        };
        // Re-read the visible view: fresh data after a change, or the offline explanation.
        self.request(self.screen)
    }

    pub(crate) fn on_loaded(&mut self, loaded: Loaded) {
        self.pending.remove(&loaded.screen);
        self.bodies.insert(loaded.screen, loaded.body);
    }
}

fn link_line(link: Link) -> Line<'static> {
    match link {
        Link::Unknown => Line::from("Connecting to the BlueRoute service…"),
        Link::Connected => Line::from("Live: views update when the daemon reports a change."),
        Link::Disconnected => Line::from("Lost contact with the BlueRoute service; retrying…")
            .style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
    }
}

const HELP: &str = "1-4 or o/n/d/i   choose a view\n\
Tab / Shift-Tab   next / previous view\n\
Up/Down, j/k      scroll; PgUp/PgDn, Home\n\
r                 refresh this view\n\
h or ?            this help (Esc closes it)\n\
q or Ctrl-C       quit\n\n\
This interface is read-only: it never changes networks or trust.";

pub(crate) fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small for BlueRoute; enlarge it or press q.")
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let [tabs, link, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(area);

    let selected = Screen::ALL
        .iter()
        .position(|screen| *screen == app.screen)
        .unwrap_or(0);
    let titles = Screen::ALL
        .iter()
        .enumerate()
        .map(|(index, screen)| format!("{} {}", index + 1, screen.title()));
    frame.render_widget(
        Tabs::new(titles)
            .select(selected)
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED))
            .block(Block::default().borders(Borders::ALL).title(" BlueRoute ")),
        tabs,
    );
    frame.render_widget(Paragraph::new(link_line(app.link)), link);

    let loading = app.pending.contains(&app.screen);
    let title = format!(
        " {}{} ",
        app.screen.title(),
        if loading { " (refreshing…)" } else { "" }
    );
    let text = app
        .bodies
        .get(&app.screen)
        .map_or("Loading…", String::as_str);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0))
            .block(Block::default().borders(Borders::ALL).title(title)),
        body,
    );
    frame.render_widget(
        Paragraph::new("1-4 views  Tab next  r refresh  ↑↓ scroll  h help  q quit"),
        footer,
    );

    if app.help {
        let popup = centered(area, 60, 11);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(HELP)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(" Help ")),
            popup,
        );
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn screen_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        screen_text(&terminal)
    }

    fn loaded(screen: Screen, body: &str) -> Loaded {
        Loaded {
            screen,
            body: body.to_owned(),
        }
    }

    #[test]
    fn starts_by_loading_the_overview_and_renders_it_when_it_arrives() {
        let mut app = App::default();
        assert_eq!(app.start(), vec![Screen::Overview]);
        let text = render(&app, 80, 20);
        assert!(text.contains("Loading…"));
        assert!(text.contains("(refreshing…)"));
        assert!(text.contains("Connecting to the BlueRoute service"));
        app.on_loaded(loaded(
            Screen::Overview,
            "Status: Healthy (local daemon only)",
        ));
        let text = render(&app, 80, 20);
        assert!(text.contains("Status: Healthy"));
        assert!(!text.contains("(refreshing…)"));
        assert!(text.contains("1 Overview") && text.contains("4 Diagnostics"));
    }

    #[test]
    fn keys_navigate_refresh_and_never_duplicate_an_in_flight_load() {
        let mut app = App::default();
        app.start();
        app.on_loaded(loaded(Screen::Overview, "o"));
        assert_eq!(app.on_key(key(KeyCode::Char('2'))), vec![Screen::Networks]);
        // Already loading: a second request for the same view is not sent.
        assert!(app.on_key(key(KeyCode::Char('n'))).is_empty());
        app.on_loaded(loaded(Screen::Networks, "Lab"));
        assert_eq!(app.on_key(key(KeyCode::Char('r'))), vec![Screen::Networks]);
        app.on_loaded(loaded(Screen::Networks, "Lab"));
        assert_eq!(app.on_key(key(KeyCode::Tab)), vec![Screen::Devices]);
        app.on_loaded(loaded(Screen::Devices, "This computer"));
        assert_eq!(app.on_key(key(KeyCode::BackTab)), vec![Screen::Networks]);
        app.on_loaded(loaded(Screen::Networks, "Lab"));
        assert_eq!(app.on_key(key(KeyCode::Left)), vec![Screen::Overview]);
        app.on_loaded(loaded(Screen::Overview, "o"));
        // Wraps around both ways.
        assert_eq!(app.on_key(key(KeyCode::Left)), vec![Screen::Diagnostics]);
        assert!(render(&app, 80, 20).contains(" Diagnostics "));
        assert!(app.on_key(key(KeyCode::Char('x'))).is_empty());
        assert!(!app.should_quit());
    }

    #[test]
    fn help_is_an_overlay_that_esc_closes_and_q_or_ctrl_c_quits() {
        let mut app = App::default();
        app.on_loaded(loaded(Screen::Overview, "body"));
        app.on_key(key(KeyCode::Char('?')));
        let text = render(&app, 80, 20);
        assert!(text.contains(" Help "));
        assert!(text.contains("read-only"));
        // Keys other than close or quit do nothing while help is open.
        assert!(app.on_key(key(KeyCode::Char('2'))).is_empty());
        app.on_key(key(KeyCode::Esc));
        assert!(!render(&app, 80, 20).contains(" Help "));
        // Esc on the main view is not quit.
        app.on_key(key(KeyCode::Esc));
        assert!(!app.should_quit());
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.should_quit());

        let mut app = App::default();
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit());
        let mut app = App::default();
        app.on_key(key(KeyCode::Char('h')));
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.should_quit());
    }

    #[test]
    fn daemon_signals_drive_the_banner_and_reload_the_visible_view() {
        let mut app = App::default();
        app.start();
        app.on_loaded(loaded(Screen::Overview, "first"));
        assert_eq!(app.on_signal(Signal::Connected), vec![Screen::Overview]);
        assert!(render(&app, 80, 20).contains("Live: views update"));
        // A burst of changes sends one load while one is in flight.
        assert!(app.on_signal(Signal::Changed).is_empty());
        app.on_loaded(loaded(Screen::Overview, "second"));
        assert!(render(&app, 80, 20).contains("second"));

        assert_eq!(app.on_signal(Signal::Disconnected), vec![Screen::Overview]);
        let text = render(&app, 80, 20);
        assert!(text.contains("Lost contact with the BlueRoute service"));
        // The last view stays visible until the reload explains what happened.
        assert!(text.contains("second"));
        app.on_loaded(loaded(
            Screen::Overview,
            "Daemon unavailable or incompatible.",
        ));
        app.on_signal(Signal::Connected);
        app.on_loaded(loaded(Screen::Overview, "restored"));
        let text = render(&app, 80, 20);
        assert!(text.contains("restored") && text.contains("Live:"));
    }

    #[test]
    fn it_redraws_at_any_size_and_explains_a_terminal_that_is_too_small() {
        let mut app = App::default();
        let long: String = (0..60).map(|line| format!("line {line}\n")).collect();
        app.on_loaded(loaded(Screen::Overview, &long));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(screen_text(&terminal).contains("line 0"));
        // Resizing: the next draw uses the new size.
        terminal.backend_mut().resize(MIN_WIDTH, MIN_HEIGHT);
        terminal.autoresize().unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(screen_text(&terminal).contains("line 0"));
        terminal.backend_mut().resize(20, 5);
        terminal.autoresize().unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(screen_text(&terminal).contains("too small"));

        // Scrolling moves the view and stops at the end of the text.
        for _ in 0..5 {
            app.on_key(key(KeyCode::Down));
        }
        let text = render(&app, 80, 20);
        assert!(text.contains("line 5") && !text.contains("line 4\n"));
        for _ in 0..20 {
            app.on_key(key(KeyCode::PageDown));
        }
        assert_eq!(app.scroll, 60);
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.scroll, 0);
        // Changing view resets the scroll.
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Char('2')));
        assert_eq!(app.scroll, 0);
    }
}
