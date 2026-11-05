//! Daemon worker: owns the D-Bus client on its own thread, refreshes on daemon events and a
//! timer, executes user requests, and reconnects with backoff after the daemon goes away.

use std::sync::mpsc::Sender;
use std::time::Duration;

use async_io::Timer;
use blueroute_client::{BlueRouteClient, ClientError};
use blueroute_protocol::{Command, Response};
use futures_lite::future::{block_on, or};

use crate::model::{DaemonMsg, Msg, Snapshot};

const REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

pub enum Request {
    Command(Command),
    Refresh,
}

pub fn spawn(to_ui: Sender<Msg>) -> async_channel::Sender<Request> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("blueroute-daemon-worker".into())
        .spawn(move || block_on(run(to_ui, rx)))
        .expect("spawn worker thread");
    tx
}

fn send(to_ui: &Sender<Msg>, msg: DaemonMsg) -> bool {
    to_ui.send(Msg::Daemon(msg)).is_ok()
}

async fn run(to_ui: Sender<Msg>, requests: async_channel::Receiver<Request>) {
    let mut backoff = BACKOFF_MIN;
    loop {
        match BlueRouteClient::connect().await {
            Ok(client) => {
                backoff = BACKOFF_MIN;
                if !send(&to_ui, DaemonMsg::Connected) {
                    return;
                }
                match session(&client, &to_ui, &requests).await {
                    SessionEnd::UiGone => return,
                    SessionEnd::DaemonLost => {
                        if !send(&to_ui, DaemonMsg::Disconnected) {
                            return;
                        }
                    }
                }
            }
            Err(_) => {
                if !send(&to_ui, DaemonMsg::Disconnected) {
                    return;
                }
            }
        }
        Timer::after(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

enum SessionEnd {
    UiGone,
    DaemonLost,
}

enum Wake {
    Request(Option<Request>),
    DaemonEvent,
    Tick,
}

async fn session(
    client: &BlueRouteClient,
    to_ui: &Sender<Msg>,
    requests: &async_channel::Receiver<Request>,
) -> SessionEnd {
    let mut events = client.events().await.ok();
    loop {
        let wake = or(
            async { Wake::Request(requests.recv().await.ok()) },
            or(
                async {
                    match events.as_mut() {
                        Some(stream) => {
                            let _ = stream.next_event().await;
                            Wake::DaemonEvent
                        }
                        None => std::future::pending().await,
                    }
                },
                async {
                    Timer::after(REFRESH_INTERVAL).await;
                    Wake::Tick
                },
            ),
        )
        .await;
        match wake {
            Wake::Request(None) => return SessionEnd::UiGone,
            Wake::Request(Some(Request::Command(command))) => {
                let message = match client.request(&command).await {
                    Ok(_) => DaemonMsg::Done(done_text(&command)),
                    Err(error) if lost(&error) => return SessionEnd::DaemonLost,
                    Err(error) => DaemonMsg::Failed(friendly(&error)),
                };
                if !send(to_ui, message) {
                    return SessionEnd::UiGone;
                }
            }
            Wake::Request(Some(Request::Refresh)) | Wake::DaemonEvent | Wake::Tick => {
                match snapshot(client).await {
                    Ok(snapshot) => {
                        if !send(to_ui, DaemonMsg::Refreshed(Box::new(snapshot))) {
                            return SessionEnd::UiGone;
                        }
                    }
                    Err(error) if lost(&error) => return SessionEnd::DaemonLost,
                    Err(error) => {
                        if !send(to_ui, DaemonMsg::Failed(friendly(&error))) {
                            return SessionEnd::UiGone;
                        }
                    }
                }
            }
        }
    }
}

async fn snapshot(client: &BlueRouteClient) -> Result<Snapshot, ClientError> {
    let status = client.status().await?;
    let networks = match client.request(&Command::ListNetworks).await? {
        Response::Networks(n) => n,
        _ => Vec::new(),
    };
    let nodes = match client.request(&Command::ListNodes).await? {
        Response::Nodes(n) => n,
        _ => Vec::new(),
    };
    let diagnostics = match client.request(&Command::GetDiagnostics).await? {
        Response::Diagnostics(d) => Some(d),
        _ => None,
    };
    Ok(Snapshot {
        status: Some(status),
        networks,
        nodes,
        diagnostics,
    })
}

/// True when the failure means the daemon is gone rather than that one request was refused.
fn lost(error: &ClientError) -> bool {
    use zbus::fdo::Error as Fdo;
    match error {
        ClientError::EventStreamClosed | ClientError::ReconnectTimeout { .. } => true,
        ClientError::Bus(zbus::Error::FDO(inner)) => matches!(
            **inner,
            Fdo::ServiceUnknown(_)
                | Fdo::NameHasNoOwner(_)
                | Fdo::Disconnected(_)
                | Fdo::NoServer(_)
        ),
        ClientError::Bus(zbus::Error::InputOutput(_) | zbus::Error::Connection(..)) => true,
        _ => false,
    }
}

fn friendly(error: &ClientError) -> String {
    use zbus::fdo::Error as Fdo;
    match error {
        ClientError::Bus(zbus::Error::FDO(inner)) => match &**inner {
            Fdo::AccessDenied(_) => {
                "You're not allowed to do that. Try again as an administrator.".into()
            }
            Fdo::NotSupported(why) => format!("This computer can't do that: {why}"),
            other => format!("That didn't work: {other}"),
        },
        other => format!("That didn't work: {other}"),
    }
}

fn done_text(command: &Command) -> String {
    match command {
        Command::CreateNetwork { .. } => "Network created.",
        Command::JoinNetwork { .. } => "Joined the network.",
        Command::LeaveNetwork => "You left the network.",
        Command::TrustPeer { .. } => "That computer may now join.",
        Command::ForgetPeer { .. } => "That computer was removed.",
        Command::StartDiscovery => "Looking for nearby networks…",
        Command::StopDiscovery => "Stopped looking.",
        _ => "Done.",
    }
    .to_owned()
}
