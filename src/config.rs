use std::{
    collections::{BTreeMap, BTreeSet},
    env::{self, VarError},
    time::Duration,
};

use reqwest::header::HeaderValue;
use url::Url;

pub struct Config {
    pub(crate) owners: BTreeMap<String, Owner>,
    pub(crate) ledger: Url,
    pub(crate) interval: Duration,
}

pub(crate) struct Owner {
    pub token: HeaderValue,
    pub scopes: BTreeSet<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|key| env::var(key))
    }

    pub fn from_lookup(read: impl Fn(&str) -> Result<String, VarError>) -> Result<Self, String> {
        const SCOPES: &str = "GITHUB_COLLECTOR_SCOPES";
        const LEDGER: &str = "GITHUB_COLLECTOR_LEDGER_URL";
        const INTERVAL: &str = "GITHUB_COLLECTOR_INTERVAL_SECONDS";
        let scopes = required(&read, SCOPES)?;
        let mut by_owner = BTreeMap::<_, BTreeSet<_>>::new();
        let invalid_scope =
            format!("{SCOPES} must be a comma-separated list of owner/* or owner/name entries");
        for scope in scopes.split(',').map(str::trim) {
            let (owner, repo) = scope.split_once('/').ok_or_else(|| invalid_scope.clone())?;
            if !valid_owner(owner) || (repo != "*" && !valid_name(repo)) {
                return Err(invalid_scope);
            }
            by_owner
                .entry(owner.to_ascii_lowercase())
                .or_default()
                .insert(repo.to_ascii_lowercase());
        }

        let mut owners = BTreeMap::new();
        for (owner, scopes) in by_owner {
            let key = format!(
                "GITHUB_COLLECTOR_TOKEN_{}",
                owner.to_ascii_uppercase().replace('-', "_")
            );
            let value =
                required(&read, &key).map_err(|error| format!("{error} for owner {owner}"))?;
            let mut token = HeaderValue::from_str(&format!("Bearer {value}"))
                .ok()
                .filter(|_| value.bytes().all(|byte| byte.is_ascii_graphic()))
                .ok_or_else(|| {
                    format!("{key} for owner {owner} must contain only visible ASCII characters without spaces")
                })?;
            token.set_sensitive(true);
            owners.insert(owner, Owner { token, scopes });
        }

        let address = required(&read, LEDGER)?;
        let ledger = Url::parse(&address)
            .ok()
            .filter(|_| is_http_origin(&address))
            .ok_or_else(|| {
                format!("{LEDGER} must be an absolute HTTP or HTTPS origin without credentials")
            })?;
        let seconds = match read(INTERVAL) {
            Err(VarError::NotPresent) => Some(300),
            Ok(value) => value.parse::<u32>().ok().filter(|seconds| *seconds > 0),
            Err(VarError::NotUnicode(_)) => None,
        }
        .ok_or_else(|| format!("{INTERVAL} must be a positive whole number of seconds"))?;

        Ok(Self {
            owners,
            ledger,
            interval: Duration::from_secs(u64::from(seconds)),
        })
    }
}

fn required(read: &impl Fn(&str) -> Result<String, VarError>, key: &str) -> Result<String, String> {
    match read(key) {
        Ok(value) if !value.is_empty() => Ok(value),
        Ok(_) | Err(VarError::NotPresent) => Err(format!("{key} is required")),
        Err(VarError::NotUnicode(_)) => Err(format!("{key} must be valid UTF-8")),
    }
}

fn valid_owner(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 39
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub(crate) fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
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
