//! Rendering. Friendly wording first; protocol/PAN jargon only on the Diagnostics screen.

use blueroute_core::{HealthLevel, NodeCapabilities, PowerState, Reachability, Sourced};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};

use crate::model::{Link, Modal, Model, Screen};

pub fn health_word(health: HealthLevel) -> &'static str {
    match health {
        HealthLevel::Healthy => "working normally",
        HealthLevel::Degraded => "working, with some limits",
        HealthLevel::Reconnecting => "reconnecting",
        HealthLevel::Error => "not working",
    }
}

fn reach_word(reach: Reachability) -> &'static str {
    match reach {
        Reachability::Local => "this computer",
        Reachability::DirectLink => "directly connected",
        Reachability::SamePanSegment => "nearby",
        Reachability::Routed => "through another computer",
        Reachability::Unreachable => "not reachable",
    }
}

/// Short, plain warnings about what this computer cannot do. Unknown never produces one.
pub fn capability_warnings(caps: &NodeCapabilities) -> Vec<&'static str> {
    let mut warnings = Vec::new();
    if caps.adapter_usable.as_ref().is_some_and(|c| !c.value) {
        warnings.push("Bluetooth isn't available on this computer.");
    }
    if caps.panu.as_ref().is_some_and(|c| !c.value) {
        warnings.push("This computer can't join networks.");
    }
    if caps.nap.as_ref().is_some_and(|c| !c.value) {
        warnings.push("This computer can't host a network (it can still join one).");
    }
    warnings
}

fn yes_no(value: Option<&Sourced<bool>>) -> String {
    match value {
        None => "unknown".into(),
        Some(v) => format!("{} ({:?})", if v.value { "yes" } else { "no" }, v.source),
    }
}

pub fn view(model: &Model, frame: &mut Frame) {
    let [tabs, body, notice, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let titles: Vec<Line> = Screen::ALL
        .iter()
        .enumerate()
        .map(|(i, s)| Line::from(format!("{} {}", i + 1, s.title())))
        .collect();
    let selected = Screen::ALL
        .iter()
        .position(|s| *s == model.screen)
        .unwrap_or(0);
    frame.render_widget(
        Tabs::new(titles)
            .select(selected)
            .block(Block::default().borders(Borders::ALL).title(" BlueRoute "))
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
        tabs,
    );

    match model.screen {
        Screen::Overview => overview(model, frame, body),
        Screen::Networks => networks(model, frame, body),
        Screen::Devices => devices(model, frame, body),
        Screen::Diagnostics => diagnostics(model, frame, body),
    }

    if let Some(text) = &model.notice {
        frame.render_widget(
            Paragraph::new(text.as_str()).style(Style::default().fg(Color::Yellow)),
            notice,
        );
    }
    frame.render_widget(Paragraph::new(footer_line(model)), footer);

    if let Some(modal) = &model.modal {
        modal_view(modal, frame);
    }
}

pub fn connection_label(link: Link) -> String {
    match link {
        Link::Connecting => "○ Connecting to BlueRoute…".into(),
        Link::Up => "● Connected".into(),
        Link::Down { attempts } => {
            format!("○ BlueRoute isn't running — retrying (attempt {attempts})")
        }
    }
}

/// Key hints for the current screen, always visible.
pub fn key_hints(model: &Model) -> &'static str {
    match model.screen {
        Screen::Overview => "Tab next · ? help · r refresh · q quit",
        Screen::Networks => {
            "↑↓ choose · Enter join · c create · l leave · d discover · Esc back · q quit"
        }
        Screen::Devices => "↑↓ choose · Enter details · t allow · f forget · Esc back · q quit",
        Screen::Diagnostics => "Tab next · Esc back · r refresh · q quit",
    }
}

fn footer_line(model: &Model) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            connection_label(model.link),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(key_hints(model), Style::default().fg(Color::DarkGray)),
    ])
}

fn overview(model: &Model, frame: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    match &model.data.status {
        None => lines.push(Line::from("Waiting for BlueRoute…")),
        Some(status) => {
            lines.push(Line::from(vec![
                Span::raw("BlueRoute is "),
                Span::styled(
                    health_word(status.health),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("."),
            ]));
            lines.push(Line::from(match status.current_network {
                Some(id) => format!("Network: {id}"),
                None => "You're not in a network yet. Press 2 to create or join one.".to_owned(),
            }));
            let count = model.data.nodes.len();
            lines.push(Line::from(format!(
                "{count} other computer{} in view.",
                if count == 1 { "" } else { "s" }
            )));
            let warnings = capability_warnings(&status.capabilities);
            if !warnings.is_empty() {
                lines.push(Line::from(""));
            }
            for warning in warnings {
                lines.push(Line::from(Span::styled(
                    format!("• {warning}"),
                    Style::default().fg(Color::Yellow),
                )));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" Overview ")),
        area,
    );
}

fn list_state(model: &Model) -> ListState {
    ListState::default().with_selected(Some(model.selected))
}

fn networks(model: &Model, frame: &mut Frame, area: Rect) {
    let title = if model.discovering {
        " Networks — looking for nearby networks… "
    } else {
        " Networks "
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    if model.data.networks.is_empty() {
        frame.render_widget(
            Paragraph::new("No networks yet. Press c to create one, or d to look for nearby ones.")
                .wrap(Wrap { trim: true })
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = model
        .data
        .networks
        .iter()
        .map(|n| {
            ListItem::new(format!(
                "{}  ·  {} member{}",
                n.name,
                n.member_count,
                if n.member_count == 1 { "" } else { "s" }
            ))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_symbol("▶ ")
            .highlight_style(Style::default().add_modifier(Modifier::BOLD)),
        area,
        &mut list_state(model),
    );
}

fn devices(model: &Model, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Devices ");
    if model.data.nodes.is_empty() {
        frame.render_widget(Paragraph::new("No other computers yet.").block(block), area);
        return;
    }
    if model.details
        && let Some((id, node)) = model.selected_node()
    {
        let lines = vec![
            Line::from(format!("Name: {}", node.name)),
            Line::from(format!("Connection: {}", reach_word(node.reachability))),
            Line::from(format!("Id: {id}")),
            Line::from(""),
            Line::from("t allow this computer to join · f forget it · Esc back"),
        ];
        frame.render_widget(Paragraph::new(lines).block(block), area);
        return;
    }
    let items: Vec<ListItem> = model
        .data
        .nodes
        .iter()
        .map(|n| ListItem::new(format!("{}  ·  {}", n.name, reach_word(n.reachability))))
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_symbol("▶ ")
            .highlight_style(Style::default().add_modifier(Modifier::BOLD)),
        area,
        &mut list_state(model),
    );
}

fn diagnostics(model: &Model, frame: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    match &model.data.diagnostics {
        None => lines.push(Line::from("No diagnostic information yet.")),
        Some(d) => {
            lines.push(Line::from(format!("Daemon API version: {}", d.api_version)));
            lines.push(Line::from(format!("Health: {}", health_word(d.health))));
            lines.push(Line::from(format!(
                "Computers in view: {}",
                d.visible_nodes
            )));
            lines.push(Line::from(""));
            lines.push(Line::from("Capabilities"));
            let c = &d.capabilities;
            for (label, value) in [
                (
                    "Bluetooth adapter usable",
                    yes_no(c.adapter_usable.as_ref()),
                ),
                ("Can join (PANU)", yes_no(c.panu.as_ref())),
                ("Can host (NAP)", yes_no(c.nap.as_ref())),
                ("Can route", yes_no(c.routing.as_ref())),
                (
                    "Network manager",
                    c.network_backend.as_ref().map_or("unknown".into(), |b| {
                        format!("{:?} ({:?})", b.value, b.source)
                    }),
                ),
                (
                    "Power",
                    c.power_state
                        .as_ref()
                        .map_or("unknown".into(), |p| match p.value {
                            PowerState::AcPowered => "plugged in".to_owned(),
                            PowerState::Battery { percent } => format!("battery {percent}%"),
                        }),
                ),
            ] {
                lines.push(Line::from(format!("  {label}: {value}")));
            }
        }
    }
    if let Some(text) = &model.notice {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("Last message: {text}")));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Diagnostics "),
        ),
        area,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn modal_view(modal: &Modal, frame: &mut Frame) {
    let (title, lines, height): (&str, Vec<Line>, u16) = match modal {
        Modal::Help => (
            " Help ",
            vec![
                Line::from("Tab / ← →   switch screens (or press 1-4)"),
                Line::from("↑ ↓ / j k   move in a list"),
                Line::from("Enter       choose · Esc  go back"),
                Line::from("r           refresh · ?  this help · q  quit"),
                Line::from(""),
                Line::from("Press any key to close."),
            ],
            9,
        ),
        Modal::CreateNetwork(name) => (
            " New network ",
            vec![
                Line::from("Name your network, then press Enter (Esc cancels):"),
                Line::from(format!("> {name}▌")),
            ],
            6,
        ),
        Modal::Confirm { question, .. } => (
            " Please confirm ",
            vec![
                Line::from(question.clone()),
                Line::from(""),
                Line::from("y = yes · n = no"),
            ],
            6,
        ),
    };
    let area = centered(frame.area(), 56, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}
