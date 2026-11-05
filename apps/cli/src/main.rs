mod app;
mod args;
mod exit;
mod render;

use std::io::{stderr, stdout};
use std::process::ExitCode;

use blueroute_client::BlueRouteClient;
use clap::Parser;
use futures_lite::future::block_on;

use crate::app::{Io, report_error, run};
use crate::args::Cli;
use crate::exit::{Exit, classify};

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // clap prints help/version to stdout and usage errors to stderr itself.
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    let (mut out, mut err) = (stdout(), stderr());
    let client = match block_on(BlueRouteClient::connect()) {
        Ok(client) => client,
        Err(error) => {
            report_error(&error, &mut err);
            return ExitCode::from(classify(&error).code());
        }
    };
    let exit = block_on(run(
        &cli,
        &client,
        &mut Io {
            out: &mut out,
            err: &mut err,
        },
    ));
    ExitCode::from(if matches!(exit, Exit::Success) {
        0
    } else {
        exit.code()
    })
}
