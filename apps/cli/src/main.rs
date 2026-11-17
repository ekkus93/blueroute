//! BlueRoute CLI commands through the shared version-checked daemon client.

mod args;
mod error_guidance;
mod failure;
mod format;
mod local_key;
mod run;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    std::process::exit(run::run(&args));
}
