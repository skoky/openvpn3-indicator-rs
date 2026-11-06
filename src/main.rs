//! openvpn3-indicator-rs – tray indicator for OpenVPN 3 Linux, written in Rust.

mod app;
mod dialogs;
mod notify;
mod openvpn3;
mod secrets;
mod status;
mod tray;

use gtk4::glib;

const USAGE: &str = "\
Usage: openvpn3-indicator-rs [OPTIONS] [PROFILE.ovpn...]

Options:
  -v, --verbose   Show informational messages
  -d, --debug     Show debug messages
      --dump      Print configurations and sessions as seen over D-Bus, then exit
  -V, --version   Show version and exit
  -h, --help      Show this help

Profiles given on the command line are offered for import.
RUST_LOG can be used for finer-grained logging.";

fn main() -> glib::ExitCode {
    let mut level = log::LevelFilter::Warn;
    let mut dump = false;
    let mut args: Vec<String> = Vec::new();
    for (i, a) in std::env::args().enumerate() {
        match a.as_str() {
            _ if i == 0 => args.push(a),
            "-V" | "--version" => {
                println!("openvpn3-indicator-rs {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            "-v" | "--verbose" => level = level.max(log::LevelFilter::Info),
            "-d" | "--debug" => level = log::LevelFilter::Debug,
            "--dump" => dump = true,
            _ => args.push(a),
        }
    }
    env_logger::Builder::new()
        .filter_level(level)
        .parse_default_env()
        .init();

    if dump { app::dump() } else { app::run(args) }
}
