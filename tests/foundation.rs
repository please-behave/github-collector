mod support;

use std::process::Command;

const TOKEN: &str = "synthetic-token";
const TOKEN_VARIABLE: &str = "GITHUB_COLLECTOR_TOKEN_MY_ORG";
const SCOPES_VARIABLE: &str = "GITHUB_COLLECTOR_SCOPES";
const LEDGER_VARIABLE: &str = "GITHUB_COLLECTOR_LEDGER_URL";

fn collector() -> Command {
    support::command(env!("CARGO_BIN_EXE_github-collector"))
}

fn assert_config_error(command: &mut Command, variable: &str, diagnostic: &str) {
    let output = command.output().expect("run collector");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostic");
    assert_eq!(stderr.trim_end(), format!("{variable} {diagnostic}"));
}

// REQ-0001
#[test]
fn missing_configuration_names_the_required_variable_and_owner() {
    assert_config_error(&mut collector(), SCOPES_VARIABLE, "is required");
    assert_config_error(
        collector().env(SCOPES_VARIABLE, "my-org/*,My-Org/repo"),
        TOKEN_VARIABLE,
        "is required for owner my-org",
    );
    assert_config_error(
        collector()
            .env(SCOPES_VARIABLE, "my-org/*,My-Org/repo,z-other/repo")
            .env(TOKEN_VARIABLE, TOKEN),
        "GITHUB_COLLECTOR_TOKEN_Z_OTHER",
        "is required for owner z-other",
    );
    assert_config_error(
        collector()
            .env(SCOPES_VARIABLE, "My-Org/*,my-org/repo")
            .env(TOKEN_VARIABLE, TOKEN),
        LEDGER_VARIABLE,
        "is required",
    );
}

// REQ-0001
#[test]
fn invalid_configuration_diagnostics_omit_supplied_values() {
    for scopes in [
        "",
        "My-Org",
        "my-org/",
        "my-org/a/b",
        "my-org/*, ",
        "../repo",
    ] {
        let diagnostic = if scopes.is_empty() {
            "is required"
        } else {
            "must be a comma-separated list of owner/* or owner/name entries"
        };
        assert_config_error(
            collector().env(SCOPES_VARIABLE, scopes),
            SCOPES_VARIABLE,
            diagnostic,
        );
    }
    assert_config_error(
        collector()
            .env(SCOPES_VARIABLE, "my-org/*")
            .env(TOKEN_VARIABLE, "synthetic secret"),
        TOKEN_VARIABLE,
        "for owner my-org must contain only visible ASCII characters without spaces",
    );
    assert_config_error(
        collector()
            .env(SCOPES_VARIABLE, "my-org/*")
            .env(TOKEN_VARIABLE, TOKEN)
            .env(LEDGER_VARIABLE, "http://ledger")
            .env("GITHUB_COLLECTOR_INTERVAL_SECONDS", "0"),
        "GITHUB_COLLECTOR_INTERVAL_SECONDS",
        "must be a positive whole number of seconds",
    );
    for url in [
        "http://user:synthetic-secret@ledger:8080",
        "http://@ledger",
        "http://ledger/?token=synthetic-secret",
        "http://ledger/#synthetic-secret",
        "http://ledger/synthetic-secret",
        "http://ledger:invalid",
        "http:ledger",
        "ledger:8080",
        "ftp://ledger",
        "http:///",
    ] {
        assert_config_error(
            collector()
                .env(SCOPES_VARIABLE, "my-org/*")
                .env(TOKEN_VARIABLE, TOKEN)
                .env(LEDGER_VARIABLE, url),
            LEDGER_VARIABLE,
            "must be an absolute HTTP or HTTPS origin without credentials",
        );
    }
}

#[cfg(unix)]
#[test]
fn shutdown_child() {
    if std::env::var_os("GITHUB_COLLECTOR_TEST_SHUTDOWN").is_none() {
        return;
    }
    use github_collector::{Collector, Config};
    let config = Config::from_env().expect("configure collector");
    let api = "http://127.0.0.1:1/".parse().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            Collector::with_github_url(config, api)
                .unwrap()
                .run_until_shutdown()
                .await
                .unwrap();
        });
}

// REQ-0002
#[cfg(unix)]
#[test]
fn valid_configuration_runs_until_sigterm_or_sigint() {
    use std::{
        io::{BufRead, BufReader, Read},
        process::Stdio,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    let timeout = Duration::from_secs(10);
    for (signal, origin) in [("-TERM", "http://ledger:8080"), ("-INT", "https://[::1]/")] {
        let mut child = support::command(std::env::current_exe().unwrap())
            .args(["--exact", "shutdown_child", "--nocapture"])
            .env("GITHUB_COLLECTOR_TEST_SHUTDOWN", "1")
            .env(SCOPES_VARIABLE, "my-org/*")
            .env(TOKEN_VARIABLE, TOKEN)
            .env(LEDGER_VARIABLE, origin)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start collector");
        let mut stderr = BufReader::new(child.stderr.take().expect("collector stderr"));
        let (ready_tx, ready_rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut first_line = String::new();
            stderr.read_line(&mut first_line).expect("read startup log");
            let _ = ready_tx.send(first_line);
            let mut remaining = String::new();
            stderr
                .read_to_string(&mut remaining)
                .expect("read shutdown log");
            remaining
        });
        let readiness = ready_rx.recv_timeout(timeout);
        if !matches!(&readiness, Ok(line) if line.trim_end() == "GitHub collector started") {
            let _ = child.kill();
            let _ = child.wait();
            panic!("collector did not start: {readiness:?}");
        }
        let sent = support::command("kill")
            .arg(signal)
            .arg(child.id().to_string())
            .status();
        if !matches!(&sent, Ok(status) if status.success()) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("could not send {signal}: {sent:?}");
        }
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait().expect("wait for collector") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("collector did not stop after {signal}");
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert!(
            status.success(),
            "collector exited with {status} after {signal}"
        );
        let logs = reader.join().unwrap();
        assert!(logs.trim_end().ends_with("GitHub collector stopped"));
        assert!(!logs.contains(TOKEN));
    }
}
