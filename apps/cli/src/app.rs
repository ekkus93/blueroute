//! CLI execution against the daemon API. Contains no Bluetooth or networking code: every
//! operation is a typed daemon command (P10-001).

use std::future::Future;
use std::io::Write;
use std::pin::Pin;

use blueroute_client::{BlueRouteClient, ClientError};
use blueroute_protocol::{Command as ApiCommand, Response};

use crate::args::{Cli, Command};
use crate::exit::{Exit, classify};

pub type DaemonFuture<'a> = Pin<Box<dyn Future<Output = Result<Response, ClientError>> + 'a>>;

/// What the CLI needs from the daemon. Implemented by the real D-Bus client and by test fakes.
pub trait Daemon {
    fn request(&self, command: ApiCommand) -> DaemonFuture<'_>;
}

impl Daemon for BlueRouteClient {
    fn request(&self, command: ApiCommand) -> DaemonFuture<'_> {
        Box::pin(async move { BlueRouteClient::request(self, &command).await })
    }
}

pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// Run one parsed command and return the process exit status.
pub async fn run(cli: &Cli, daemon: &dyn Daemon, io: &mut Io<'_>) -> Exit {
    let result = match &cli.command {
        Command::Status => daemon.request(ApiCommand::GetStatus).await,
        _ => {
            let _ = writeln!(io.err, "blueroute: this command is not available yet");
            return Exit::Unsupported;
        }
    };
    match result {
        Ok(response) => {
            crate::render::render(&cli.command, cli.json, &response, io.out);
            Exit::Success
        }
        Err(error) => {
            report_error(&error, io.err);
            classify(&error)
        }
    }
}

pub fn report_error(error: &ClientError, err: &mut dyn Write) {
    let exit = classify(error);
    let hint = match exit {
        Exit::DaemonUnavailable => {
            "\nIs the BlueRoute daemon running? Try: systemctl status blueroute"
        }
        Exit::Unauthorized => "\nYou may need to authenticate or run this as an administrator.",
        _ => "",
    };
    let _ = writeln!(err, "blueroute: {error}{hint}");
}

#[cfg(test)]
pub mod testing {
    use std::cell::RefCell;

    use super::*;

    /// Scripted daemon: returns queued results and records every command it receives.
    pub struct FakeDaemon {
        pub replies: RefCell<Vec<Result<Response, ClientError>>>,
        pub seen: RefCell<Vec<ApiCommand>>,
    }

    impl FakeDaemon {
        pub fn replying(replies: Vec<Result<Response, ClientError>>) -> Self {
            Self {
                replies: RefCell::new(replies.into_iter().rev().collect()),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl Daemon for FakeDaemon {
        fn request(&self, command: ApiCommand) -> DaemonFuture<'_> {
            self.seen.borrow_mut().push(command);
            let reply = self
                .replies
                .borrow_mut()
                .pop()
                .expect("unexpected extra request");
            Box::pin(async move { reply })
        }
    }

    pub fn run_args(args: &[&str], daemon: &FakeDaemon) -> (Exit, String, String) {
        use clap::Parser;
        let cli = Cli::try_parse_from(args).expect("test arguments parse");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let exit = futures_lite::future::block_on(run(
            &cli,
            daemon,
            &mut Io {
                out: &mut out,
                err: &mut err,
            },
        ));
        (
            exit,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }
}

#[cfg(test)]
mod tests {
    use blueroute_core::{HealthLevel, NodeCapabilities};
    use blueroute_protocol::{ApiVersion, DaemonStatus};
    use zbus::fdo;

    use super::testing::*;
    use super::*;

    fn status() -> Response {
        Response::Status(DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: None,
            current_network: None,
            health: HealthLevel::Healthy,
            capabilities: NodeCapabilities::default(),
        })
    }

    #[test]
    fn status_succeeds_in_text_and_json() {
        let daemon = FakeDaemon::replying(vec![Ok(status()), Ok(status())]);
        let (exit, out, _) = run_args(&["blueroute", "status"], &daemon);
        assert_eq!(exit, Exit::Success);
        assert!(out.contains("working normally") && out.contains("not in a network"));
        let (_, json, _) = run_args(&["blueroute", "--json", "status"], &daemon);
        let value: serde_json::Value = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(value["type"], "status");
        assert_eq!(
            daemon.seen.borrow().as_slice(),
            [ApiCommand::GetStatus, ApiCommand::GetStatus]
        );
    }

    #[test]
    fn daemon_errors_set_the_documented_exit_status_and_explain() {
        let cases = [
            (fdo::Error::AccessDenied("no".into()), Exit::Unauthorized),
            (
                fdo::Error::NotSupported("no adapter".into()),
                Exit::Unsupported,
            ),
            (
                fdo::Error::ServiceUnknown("gone".into()),
                Exit::DaemonUnavailable,
            ),
            (fdo::Error::Failed("boom".into()), Exit::OperationFailed),
        ];
        for (error, expected) in cases {
            let daemon = FakeDaemon::replying(vec![Err(ClientError::Bus(zbus::Error::FDO(
                Box::new(error),
            )))]);
            let (exit, out, err) = run_args(&["blueroute", "status"], &daemon);
            assert_eq!(exit, expected);
            assert!(out.is_empty());
            assert!(err.starts_with("blueroute: "), "{err}");
        }
    }

    #[test]
    fn unavailable_daemon_message_suggests_the_next_step() {
        let daemon = FakeDaemon::replying(vec![Err(ClientError::Bus(zbus::Error::FDO(Box::new(
            fdo::Error::ServiceUnknown("x".into()),
        ))))]);
        let (_, _, err) = run_args(&["blueroute", "status"], &daemon);
        assert!(err.contains("daemon running"));
    }
}
