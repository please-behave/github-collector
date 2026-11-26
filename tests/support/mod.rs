use std::{env, ffi::OsStr, process::Command};

pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("GITHUB_COLLECTOR_") {
            command.env_remove(key);
        }
    }
    command
}
