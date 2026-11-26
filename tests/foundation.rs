mod support;

use std::process::Command;

const TOKEN: &str = "synthetic-token";

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
fn missing_configuration_names_the_required_variable() {
    assert_config_error(&mut collector(), "GITHUB_COLLECTOR_TOKEN", "is required");
    assert_config_error(
        collector().env("GITHUB_COLLECTOR_TOKEN", TOKEN),
        "GITHUB_COLLECTOR_LEDGER_URL",
        "is required",
    );
}

// REQ-0001
#[test]
fn invalid_configuration_diagnostics_omit_supplied_values() {
    assert_config_error(
        collector().env("GITHUB_COLLECTOR_TOKEN", "synthetic secret"),
        "GITHUB_COLLECTOR_TOKEN",
        "must contain only visible ASCII characters without spaces",
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
                .env("GITHUB_COLLECTOR_TOKEN", TOKEN)
                .env("GITHUB_COLLECTOR_LEDGER_URL", url),
            "GITHUB_COLLECTOR_LEDGER_URL",
            "must be an absolute HTTP or HTTPS origin without credentials",
        );
    }
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
        let mut child = collector()
            .env("GITHUB_COLLECTOR_TOKEN", TOKEN)
            .env("GITHUB_COLLECTOR_LEDGER_URL", origin)
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
        assert_eq!(
            reader.join().unwrap().trim_end(),
            "GitHub collector stopped"
        );
    }
}
