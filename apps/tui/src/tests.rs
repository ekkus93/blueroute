//! Model and rendering tests. Rendering uses ratatui's in-memory backend, so no terminal or
//! daemon is needed (P11-001..006).

use blueroute_core::{
    CapabilitySource, DisplayName, HealthLevel, NetworkId, NodeCapabilities, NodeId, Reachability,
    Sourced,
};
use blueroute_protocol::{
    ApiVersion, Command, DaemonStatus, DiagnosticSnapshot, NetworkSummary, NodeSummary,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::model::{DaemonMsg, Effect, Key, Link, Modal, Model, Msg, Screen, Snapshot};
use crate::view::view;

fn caps(nap: Option<bool>) -> NodeCapabilities {
    NodeCapabilities {
        nap: nap.map(|v| Sourced::new(v, CapabilitySource::Discovered)),
        ..NodeCapabilities::default()
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        status: Some(DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: Some(NodeId::from_bytes([1; 16])),
            current_network: None,
            health: HealthLevel::Degraded,
            capabilities: caps(Some(false)),
        }),
        networks: vec![
            NetworkSummary {
                id: NetworkId::from_bytes([5; 16]),
                name: DisplayName::new("Home").unwrap(),
                member_count: 2,
            },
            NetworkSummary {
                id: NetworkId::from_bytes([6; 16]),
                name: DisplayName::new("Office").unwrap(),
                member_count: 1,
            },
        ],
        nodes: vec![
            NodeSummary {
                id: NodeId::from_bytes([7; 16]),
                name: DisplayName::new("Kitchen laptop").unwrap(),
                reachability: Reachability::Routed,
            },
            NodeSummary {
                id: NodeId::from_bytes([8; 16]),
                name: DisplayName::new("Desk PC").unwrap(),
                reachability: Reachability::DirectLink,
            },
        ],
        diagnostics: Some(DiagnosticSnapshot {
            api_version: ApiVersion::CURRENT,
            health: HealthLevel::Degraded,
            current_network: None,
            visible_nodes: 2,
            capabilities: caps(Some(false)),
        }),
    }
}

fn connected() -> Model {
    let mut m = Model::default();
    m.update(Msg::Daemon(DaemonMsg::Connected));
    m.update(Msg::Daemon(DaemonMsg::Refreshed(Box::new(snapshot()))));
    m
}

fn key(m: &mut Model, k: Key) -> Vec<Effect> {
    m.update(Msg::Key(k))
}

fn type_str(m: &mut Model, text: &str) {
    for c in text.chars() {
        key(m, Key::Char(c));
    }
}

fn render(m: &Model, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| view(m, f)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn starts_connecting_and_renders_status_once_data_arrives() {
    let m = Model::default();
    assert!(render(&m, 80, 24).contains("Connecting to BlueRoute"));
    let m = connected();
    let text = render(&m, 80, 24);
    assert!(text.contains("BlueRoute is working, with some limits."));
    assert!(text.contains("not in a network yet"));
    assert!(text.contains("2 other computers in view."));
    assert!(text.contains("● Connected"));
}

#[test]
fn connecting_requests_a_refresh() {
    let mut m = Model::default();
    assert_eq!(
        m.update(Msg::Daemon(DaemonMsg::Connected)),
        vec![Effect::Refresh]
    );
}

#[test]
fn daemon_disconnect_keeps_data_shows_retry_and_reconnect_restores() {
    let mut m = connected();
    m.update(Msg::Daemon(DaemonMsg::Disconnected));
    m.update(Msg::Daemon(DaemonMsg::Disconnected));
    assert_eq!(m.link, Link::Down { attempts: 2 });
    let text = render(&m, 100, 24);
    assert!(text.contains("isn't running") && text.contains("attempt 2"));
    assert!(
        text.contains("2 other computers"),
        "last known data stays visible"
    );
    // Actions are refused while down.
    key(&mut m, Key::Char('2'));
    assert!(key(&mut m, Key::Enter).is_empty());
    assert!(m.notice.as_deref().unwrap().contains("Not connected"));
    assert_eq!(
        m.update(Msg::Daemon(DaemonMsg::Connected)),
        vec![Effect::Refresh]
    );
    assert_eq!(m.link, Link::Up);
    assert!(m.notice.is_none());
}

#[test]
fn resize_is_tracked_and_tiny_terminals_do_not_panic() {
    let mut m = connected();
    m.update(Msg::Resize(30, 8));
    assert_eq!(m.size, (30, 8));
    for screen in ['1', '2', '3', '4'] {
        key(&mut m, Key::Char(screen));
        for (w, h) in [(30, 8), (10, 4), (1, 1), (120, 40)] {
            let _ = render(&m, w, h);
        }
    }
    key(&mut m, Key::Char('?'));
    let _ = render(&m, 20, 6);
}

#[test]
fn overview_uses_friendly_words_warns_plainly_and_hides_internet() {
    let m = connected();
    let text = render(&m, 90, 24);
    assert!(text.contains("This computer can't host a network (it can still join one)."));
    for jargon in ["NAP", "PANU", "BNEP", "Internet"] {
        assert!(!text.contains(jargon), "overview mentions {jargon}");
    }
    let mut unknown = connected();
    unknown.data.status.as_mut().unwrap().capabilities = caps(None);
    assert!(
        !render(&unknown, 90, 24).contains("can't"),
        "unknown capabilities are not warnings"
    );
}

#[test]
fn navigation_is_predictable_and_works_without_a_mouse() {
    let mut m = connected();
    key(&mut m, Key::Tab);
    assert_eq!(m.screen, Screen::Networks);
    key(&mut m, Key::Right);
    assert_eq!(m.screen, Screen::Devices);
    key(&mut m, Key::BackTab);
    key(&mut m, Key::Left);
    assert_eq!(m.screen, Screen::Overview);
    key(&mut m, Key::Left);
    assert_eq!(m.screen, Screen::Diagnostics, "wraps around");
    key(&mut m, Key::Char('3'));
    key(&mut m, Key::Esc);
    assert_eq!(m.screen, Screen::Overview, "Esc goes back to the start");
    assert!(key(&mut m, Key::Esc).is_empty());
    assert_eq!(key(&mut m, Key::Char('q')), vec![Effect::Quit]);
    assert_eq!(key(&mut m, Key::Char('r')), vec![Effect::Refresh]);
}

#[test]
fn q_quits_from_every_screen_and_help_closes_on_any_key() {
    for screen in ['1', '2', '3', '4'] {
        let mut m = connected();
        key(&mut m, Key::Char(screen));
        assert_eq!(key(&mut m, Key::Char('q')), vec![Effect::Quit]);
    }
    let mut m = connected();
    key(&mut m, Key::Char('?'));
    assert_eq!(m.modal, Some(Modal::Help));
    assert!(render(&m, 80, 24).contains("Press any key to close"));
    assert!(key(&mut m, Key::Char('x')).is_empty());
    assert!(m.modal.is_none());
}

#[test]
fn every_screen_shows_key_hints() {
    let mut m = connected();
    for (k, hint) in [
        ('1', "q quit"),
        ('2', "c create"),
        ('3', "t allow"),
        ('4', "Esc back"),
    ] {
        key(&mut m, Key::Char(k));
        assert!(
            render(&m, 110, 24).contains(hint),
            "screen {k} lacks hint {hint:?}"
        );
    }
}

#[test]
fn networks_screen_lists_and_runs_the_single_star_workflow() {
    let mut m = connected();
    key(&mut m, Key::Char('2'));
    let text = render(&m, 90, 24);
    assert!(text.contains("Home") && text.contains("2 members") && text.contains("Office"));

    // Join the selected network.
    key(&mut m, Key::Down);
    assert_eq!(
        key(&mut m, Key::Enter),
        vec![Effect::Request(Command::JoinNetwork {
            network: NetworkId::from_bytes([6; 16])
        })]
    );

    // Create: type a name, Enter submits, Esc cancels, empty name is refused.
    key(&mut m, Key::Char('c'));
    assert!(render(&m, 90, 24).contains("Name your network"));
    type_str(&mut m, "Lab");
    key(&mut m, Key::Backspace);
    type_str(&mut m, "b!");
    assert_eq!(
        key(&mut m, Key::Enter),
        vec![Effect::Request(Command::CreateNetwork {
            name: DisplayName::new("Lab!").unwrap()
        })]
    );
    key(&mut m, Key::Char('c'));
    assert!(key(&mut m, Key::Enter).is_empty());
    assert!(m.notice.as_deref().unwrap().contains("type a name"));
    key(&mut m, Key::Char('c'));
    key(&mut m, Key::Esc);
    assert!(m.modal.is_none());

    // Leave needs confirmation; n cancels, y proceeds.
    key(&mut m, Key::Char('l'));
    assert!(render(&m, 90, 24).contains("Leave the current network?"));
    assert!(key(&mut m, Key::Char('n')).is_empty());
    key(&mut m, Key::Char('l'));
    assert_eq!(
        key(&mut m, Key::Char('y')),
        vec![Effect::Request(Command::LeaveNetwork)]
    );

    // Discover toggles on and off.
    assert_eq!(
        key(&mut m, Key::Char('d')),
        vec![Effect::Request(Command::StartDiscovery)]
    );
    assert!(render(&m, 90, 24).contains("looking for nearby"));
    assert_eq!(
        key(&mut m, Key::Char('d')),
        vec![Effect::Request(Command::StopDiscovery)]
    );
}

#[test]
fn empty_networks_screen_explains_what_to_do() {
    let mut m = connected();
    m.update(Msg::Daemon(DaemonMsg::Refreshed(Box::default())));
    key(&mut m, Key::Char('2'));
    assert!(render(&m, 90, 24).contains("Press c to create one"));
    assert!(key(&mut m, Key::Enter).is_empty(), "nothing to join");
}

#[test]
fn devices_show_plain_connection_wording_details_and_confirmed_trust_actions() {
    let mut m = connected();
    key(&mut m, Key::Char('3'));
    let text = render(&m, 90, 24);
    assert!(text.contains("Kitchen laptop") && text.contains("through another computer"));
    assert!(text.contains("Desk PC") && text.contains("directly connected"));
    for jargon in ["PANU", "NAP", "BNEP", "Routed", "DirectLink"] {
        assert!(!text.contains(jargon), "devices screen mentions {jargon}");
    }
    key(&mut m, Key::Enter);
    let details = render(&m, 90, 24);
    assert!(
        details.contains("Name: Kitchen laptop")
            && details.contains(&NodeId::from_bytes([7; 16]).to_string())
    );
    key(&mut m, Key::Esc);
    assert!(
        !m.details && m.screen == Screen::Devices,
        "Esc closes details first"
    );

    // Trust is never immediate: it asks, and only y sends the command.
    assert!(key(&mut m, Key::Char('t')).is_empty());
    assert!(render(&m, 90, 24).contains("Allow Kitchen laptop to join your network?"));
    assert!(key(&mut m, Key::Char('n')).is_empty());
    key(&mut m, Key::Char('t'));
    assert_eq!(
        key(&mut m, Key::Char('y')),
        vec![Effect::Request(Command::TrustPeer {
            node: NodeId::from_bytes([7; 16])
        })]
    );
    key(&mut m, Key::Down);
    key(&mut m, Key::Char('f'));
    assert_eq!(
        key(&mut m, Key::Char('y')),
        vec![Effect::Request(Command::ForgetPeer {
            node: NodeId::from_bytes([8; 16])
        })]
    );
    // Any other key at the prompt keeps it open; it never acts by accident.
    key(&mut m, Key::Char('t'));
    assert!(key(&mut m, Key::Char('x')).is_empty());
    assert!(matches!(m.modal, Some(Modal::Confirm { .. })));
}

#[test]
fn diagnostics_keeps_technical_detail_off_the_primary_screens() {
    let mut m = connected();
    let overview = render(&m, 100, 30);
    key(&mut m, Key::Char('4'));
    let text = render(&m, 100, 30);
    assert!(text.contains("Daemon API version: 1.0"));
    assert!(text.contains("Can host (NAP): no (Discovered)"));
    assert!(text.contains("Can route: unknown"));
    assert!(!overview.contains("Daemon API version"));
}

#[test]
fn selection_is_clamped_when_the_list_shrinks_and_action_results_surface() {
    let mut m = connected();
    key(&mut m, Key::Char('3'));
    key(&mut m, Key::Down);
    assert_eq!(m.selected, 1);
    let mut one = snapshot();
    one.nodes.truncate(1);
    m.update(Msg::Daemon(DaemonMsg::Refreshed(Box::new(one))));
    assert_eq!(m.selected, 0);
    assert_eq!(
        m.update(Msg::Daemon(DaemonMsg::Done("Network created.".into()))),
        vec![Effect::Refresh]
    );
    assert!(render(&m, 90, 24).contains("Network created."));
    m.update(Msg::Daemon(DaemonMsg::Failed("That didn't work: x".into())));
    assert!(render(&m, 90, 24).contains("That didn't work"));
}
