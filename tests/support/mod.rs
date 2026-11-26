use std::{ffi::OsStr, process::Command};

pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.env_remove("GITHUB_COLLECTOR_TOKEN");
    command.env_remove("GITHUB_COLLECTOR_LEDGER_URL");
    command
}
