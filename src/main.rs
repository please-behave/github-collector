use std::{env, io, process::ExitCode};

use url::Url;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    if let Err(message) = validate_config() {
        eprintln!("{message}");
        return ExitCode::FAILURE;
    }
    if let Err(error) = wait_for_shutdown().await {
        eprintln!("Could not listen for shutdown signals: {error}");
        return ExitCode::FAILURE;
    }
    eprintln!("GitHub collector stopped");
    ExitCode::SUCCESS
}

fn validate_config() -> Result<(), &'static str> {
    let token = required_env(
        "GITHUB_COLLECTOR_TOKEN",
        "GITHUB_COLLECTOR_TOKEN is required",
        "GITHUB_COLLECTOR_TOKEN must contain only visible ASCII characters without spaces",
    )?;
    if !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(
            "GITHUB_COLLECTOR_TOKEN must contain only visible ASCII characters without spaces",
        );
    }

    let invalid_origin =
        "GITHUB_COLLECTOR_LEDGER_URL must be an absolute HTTP or HTTPS origin without credentials";
    let ledger_url = required_env(
        "GITHUB_COLLECTOR_LEDGER_URL",
        "GITHUB_COLLECTOR_LEDGER_URL is required",
        invalid_origin,
    )?;
    if !is_http_origin(&ledger_url) {
        return Err(invalid_origin);
    }
    Ok(())
}

fn required_env(
    name: &str,
    missing: &'static str,
    invalid: &'static str,
) -> Result<String, &'static str> {
    match env::var(name) {
        Ok(value) if value.is_empty() => Err(missing),
        Ok(value) => Ok(value),
        Err(env::VarError::NotPresent) => Err(missing),
        Err(env::VarError::NotUnicode(_)) => Err(invalid),
    }
}

fn is_http_origin(value: &str) -> bool {
    let Some((scheme, authority)) = value.split_once("://") else {
        return false;
    };
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
        && !authority.contains(['/', '\\', '@', '?', '#'])
        && !authority
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        && Url::parse(value).is_ok_and(|url| url.has_host())
}

#[cfg(unix)]
async fn wait_for_shutdown() -> io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    eprintln!("GitHub collector started");
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
    Ok(())
}

#[cfg(windows)]
async fn wait_for_shutdown() -> io::Result<()> {
    let mut interrupt = tokio::signal::windows::ctrl_c()?;
    eprintln!("GitHub collector started");
    interrupt.recv().await;
    Ok(())
}
