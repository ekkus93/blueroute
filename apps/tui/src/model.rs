//! TUI state and pure update logic (P11-001..006). No terminal or daemon access here, so every
//! behavior is unit-testable. Effects are returned for the runtime to perform.

use blueroute_core::{DisplayName, NodeId};
use blueroute_protocol::{Command, DaemonStatus, DiagnosticSnapshot, NetworkSummary, NodeSummary};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Screen {
    Overview,
    Networks,
    Devices,
    Diagnostics,
}

impl Screen {
    pub const ALL: [Self; 4] = [
        Self::Overview,
        Self::Networks,
        Self::Devices,
        Self::Diagnostics,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Networks => "Networks",
            Self::Devices => "Devices",
            Self::Diagnostics => "Diagnostics",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Link {
    Connecting,
    Up,
    /// Daemon unreachable; the worker keeps retrying.
    Down {
        attempts: u32,
    },
}

/// Terminal-independent key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    Char(char),
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
    Backspace,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub status: Option<DaemonStatus>,
    pub networks: Vec<NetworkSummary>,
    pub nodes: Vec<NodeSummary>,
    pub diagnostics: Option<DiagnosticSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DaemonMsg {
    Connected,
    Disconnected,
    Refreshed(Box<Snapshot>),
    Done(String),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    Key(Key),
    Resize(u16, u16),
    Daemon(DaemonMsg),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Request(Command),
    Refresh,
    Quit,
}

/// A modal prompt shown over the current screen.
#[derive(Clone, Debug, PartialEq)]
pub enum Modal {
    Help,
    /// Typing a new network name.
    CreateNetwork(String),
    /// Waiting for y/n before running `command`.
    Confirm {
        question: String,
        command: Command,
    },
}

#[derive(Debug)]
pub struct Model {
    pub link: Link,
    pub screen: Screen,
    pub data: Snapshot,
    pub selected: usize,
    pub details: bool,
    pub discovering: bool,
    pub notice: Option<String>,
    pub modal: Option<Modal>,
    pub size: (u16, u16),
}

impl Default for Model {
    fn default() -> Self {
        Self {
            link: Link::Connecting,
            screen: Screen::Overview,
            data: Snapshot::default(),
            selected: 0,
            details: false,
            discovering: false,
            notice: None,
            modal: None,
            size: (80, 24),
        }
    }
}

impl Model {
    fn list_len(&self) -> usize {
        match self.screen {
            Screen::Networks => self.data.networks.len(),
            Screen::Devices => self.data.nodes.len(),
            _ => 0,
        }
    }

    fn go(&mut self, screen: Screen) {
        self.screen = screen;
        self.selected = 0;
        self.details = false;
    }

    fn cycle(&mut self, step: isize) {
        let n = Screen::ALL.len() as isize;
        let next = (self.screen.index() as isize + step).rem_euclid(n) as usize;
        self.go(Screen::ALL[next]);
    }

    fn connected(&self) -> bool {
        self.link == Link::Up
    }

    fn refuse_if_down(&mut self) -> bool {
        if self.connected() {
            false
        } else {
            self.notice = Some("Not connected to BlueRoute yet. Waiting for it to start…".into());
            true
        }
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Resize(w, h) => {
                self.size = (w, h);
                Vec::new()
            }
            Msg::Daemon(msg) => self.on_daemon(msg),
            Msg::Key(key) => self.on_key(key),
        }
    }

    fn on_daemon(&mut self, msg: DaemonMsg) -> Vec<Effect> {
        match msg {
            DaemonMsg::Connected => {
                self.link = Link::Up;
                self.notice = None;
                vec![Effect::Refresh]
            }
            DaemonMsg::Disconnected => {
                let attempts = match self.link {
                    Link::Down { attempts } => attempts + 1,
                    _ => 1,
                };
                self.link = Link::Down { attempts };
                Vec::new()
            }
            DaemonMsg::Refreshed(snapshot) => {
                self.link = Link::Up;
                self.data = *snapshot;
                self.selected = self.selected.min(self.list_len().saturating_sub(1));
                Vec::new()
            }
            DaemonMsg::Done(text) => {
                self.notice = Some(text);
                vec![Effect::Refresh]
            }
            DaemonMsg::Failed(text) => {
                self.notice = Some(text);
                Vec::new()
            }
        }
    }

    fn on_key(&mut self, key: Key) -> Vec<Effect> {
        if let Some(modal) = self.modal.take() {
            return self.on_modal_key(modal, key);
        }
        self.notice = None;
        match key {
            Key::Char('q') => return vec![Effect::Quit],
            Key::Char('?') => self.modal = Some(Modal::Help),
            Key::Tab | Key::Right => self.cycle(1),
            Key::BackTab | Key::Left => self.cycle(-1),
            Key::Char(c @ '1'..='4') => self.go(Screen::ALL[usize::from(c as u8 - b'1')]),
            Key::Esc => {
                if self.details {
                    self.details = false;
                } else if self.screen != Screen::Overview {
                    self.go(Screen::Overview);
                }
            }
            Key::Up | Key::Char('k') => self.selected = self.selected.saturating_sub(1),
            Key::Down | Key::Char('j') => {
                self.selected = (self.selected + 1).min(self.list_len().saturating_sub(1));
            }
            Key::Char('r') => return vec![Effect::Refresh],
            other => return self.screen_key(other),
        }
        Vec::new()
    }

    fn on_modal_key(&mut self, modal: Modal, key: Key) -> Vec<Effect> {
        match (modal, key) {
            (Modal::Help, _) => Vec::new(),
            (Modal::CreateNetwork(_), Key::Esc) => Vec::new(),
            (Modal::CreateNetwork(name), Key::Enter) => match DisplayName::new(name.trim()) {
                Ok(name) => vec![Effect::Request(Command::CreateNetwork { name })],
                Err(_) => {
                    self.notice = Some("Please type a name for the network.".into());
                    Vec::new()
                }
            },
            (Modal::CreateNetwork(mut name), Key::Backspace) => {
                name.pop();
                self.modal = Some(Modal::CreateNetwork(name));
                Vec::new()
            }
            (Modal::CreateNetwork(mut name), Key::Char(c)) if name.chars().count() < 40 => {
                name.push(c);
                self.modal = Some(Modal::CreateNetwork(name));
                Vec::new()
            }
            (modal @ Modal::CreateNetwork(_), _) => {
                self.modal = Some(modal);
                Vec::new()
            }
            (Modal::Confirm { command, .. }, Key::Char('y' | 'Y')) => {
                vec![Effect::Request(command)]
            }
            (Modal::Confirm { .. }, Key::Char('n' | 'N') | Key::Esc) => Vec::new(),
            (modal @ Modal::Confirm { .. }, _) => {
                self.modal = Some(modal);
                Vec::new()
            }
        }
    }

    fn screen_key(&mut self, key: Key) -> Vec<Effect> {
        match (self.screen, key) {
            (Screen::Networks, Key::Char('c')) if !self.refuse_if_down() => {
                self.modal = Some(Modal::CreateNetwork(String::new()));
            }
            (Screen::Networks, Key::Enter) => {
                if self.refuse_if_down() {
                    return Vec::new();
                }
                if let Some(network) = self.data.networks.get(self.selected) {
                    return vec![Effect::Request(Command::JoinNetwork {
                        network: network.id,
                    })];
                }
            }
            (Screen::Networks, Key::Char('l')) if !self.refuse_if_down() => {
                self.modal = Some(Modal::Confirm {
                    question: "Leave the current network?".into(),
                    command: Command::LeaveNetwork,
                });
            }
            (Screen::Networks, Key::Char('d')) => {
                if self.refuse_if_down() {
                    return Vec::new();
                }
                self.discovering = !self.discovering;
                let command = if self.discovering {
                    Command::StartDiscovery
                } else {
                    Command::StopDiscovery
                };
                return vec![Effect::Request(command)];
            }
            (Screen::Devices, Key::Enter) => self.details = !self.details,
            (Screen::Devices, Key::Char('t')) => self.confirm_trust(true),
            (Screen::Devices, Key::Char('f')) => self.confirm_trust(false),
            _ => {}
        }
        Vec::new()
    }

    fn confirm_trust(&mut self, allow: bool) {
        if self.refuse_if_down() {
            return;
        }
        let Some(node) = self.data.nodes.get(self.selected) else {
            return;
        };
        let (question, command) = if allow {
            (
                format!("Allow {} to join your network?", node.name),
                Command::TrustPeer { node: node.id },
            )
        } else {
            (
                format!("Remove {} and forget it?", node.name),
                Command::ForgetPeer { node: node.id },
            )
        };
        self.modal = Some(Modal::Confirm { question, command });
    }

    pub fn selected_node(&self) -> Option<(NodeId, &NodeSummary)> {
        self.data.nodes.get(self.selected).map(|n| (n.id, n))
    }
}
