use std::process::ExitCode;

use github_collector::{Collector, Config};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let Ok(mut collector) = Collector::new(config) else {
        eprintln!("Could not initialize the HTTP client");
        return ExitCode::FAILURE;
    };

    if let Err(error) = collector.run_until_shutdown().await {
        eprintln!("Could not listen for shutdown signals: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
