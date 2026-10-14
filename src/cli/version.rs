use std::process::ExitCode;

pub fn run() -> anyhow::Result<ExitCode> {
    println!("subweave {}", env!("CARGO_PKG_VERSION"));
    Ok(ExitCode::SUCCESS)
}
