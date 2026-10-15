//! The daemon's and the dashboard's own log, on stderr: JSON lines for journald, compact
//! text at a terminal. `SUBWEAVE_LOG` filters it like `RUST_LOG`; the default is `info`.

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

pub fn init() {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .with_env_var("SUBWEAVE_LOG")
        .from_env_lossy();
    let fmt = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    if std::io::stderr().is_terminal() {
        fmt.compact().init();
    } else {
        fmt.json().flatten_event(true).init();
    }
}
