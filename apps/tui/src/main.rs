//! Minimal text-terminal shell. The daemon-backed Ratatui views are tracked in P11.
//! This foundation deliberately performs no privileged operations.
use std::io::{self, BufRead, IsTerminal, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Overview,
    Networks,
    Devices,
    Diagnostics,
}

impl Screen {
    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Networks => "Networks",
            Self::Devices => "Devices",
            Self::Diagnostics => "Diagnostics",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Show(Screen),
    Help,
    Quit,
    Invalid,
}

fn parse_action(input: &str) -> Action {
    match input.trim().to_ascii_lowercase().as_str() {
        "1" | "o" => Action::Show(Screen::Overview),
        "2" | "n" => Action::Show(Screen::Networks),
        "3" | "d" => Action::Show(Screen::Devices),
        "4" | "i" => Action::Show(Screen::Diagnostics),
        "h" | "?" => Action::Help,
        "q" | "quit" | "exit" => Action::Quit,
        _ => Action::Invalid,
    }
}

fn render(screen: Screen) -> String {
    format!(
        "BlueRoute | {}\n\nDaemon data not yet connected. No network changes are available here.\n\n1 Overview   2 Networks   3 Devices   4 Diagnostics\nh Help       q Quit\n",
        screen.title()
    )
}

fn run<R: BufRead, W: Write>(input: &mut R, output: &mut W) -> io::Result<()> {
    let mut screen = Screen::Overview;
    writeln!(output, "{}", render(screen))?;
    loop {
        write!(output, "> ")?;
        output.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            break;
        }
        match parse_action(&line) {
            Action::Quit => break,
            Action::Show(next) => {
                screen = next;
                writeln!(output, "{}", render(screen))?;
            }
            Action::Help => writeln!(output, "Select a numbered view; q quits.")?,
            Action::Invalid => writeln!(output, "Unknown key. Press h for help.")?,
        }
    }
    Ok(())
}

fn main() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("BlueRoute TUI requires an interactive text terminal.");
        std::process::exit(2);
    }
    run(&mut io::stdin().lock(), &mut io::stdout().lock())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_is_case_insensitive_and_safe() {
        assert_eq!(parse_action(" N\n"), Action::Show(Screen::Networks));
        assert_eq!(parse_action("4"), Action::Show(Screen::Diagnostics));
        assert_eq!(parse_action("quit"), Action::Quit);
        assert_eq!(parse_action("network create"), Action::Invalid);
    }
    #[test]
    fn shell_navigation_and_eof() {
        let mut output = Vec::new();
        run(&mut &b"2\n3\nh\nq\n"[..], &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("BlueRoute | Overview"));
        assert!(text.contains("BlueRoute | Networks"));
        assert!(text.contains("BlueRoute | Devices"));
        assert!(text.contains("Select a numbered view"));
        assert!(!text.contains("BlueRoute | Diagnostics"));
    }
    #[test]
    fn view_discloses_unavailable_daemon_data() {
        for screen in [
            Screen::Overview,
            Screen::Networks,
            Screen::Devices,
            Screen::Diagnostics,
        ] {
            assert!(render(screen).contains("Daemon data not yet connected"));
        }
    }
}
