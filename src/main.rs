use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    subweave::cli::main().await
}
