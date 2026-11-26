use std::{collections::BTreeMap, io, time::Duration};

use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::{Client, StatusCode};
use serde::{Serialize, Serializer};
use url::Url;

use crate::{
    config::{Config, Owner},
    github::{GitHub, Repository},
};

pub struct Collector {
    config: Config,
    client: Client,
    api: Url,
    owners: BTreeMap<String, OwnerState>,
}

impl Collector {
    pub fn new(config: Config) -> Result<Self, reqwest::Error> {
        Self::with_github_url(config, Url::parse("https://api.github.com/").unwrap())
    }

    /// Supply a GitHub API origin for an offline test.
    pub fn with_github_url(config: Config, api: Url) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .user_agent(concat!("github-collector/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            config,
            client,
            api,
            owners: BTreeMap::new(),
        })
    }

    pub async fn run_until_shutdown(&mut self) -> io::Result<()> {
        let shutdown = shutdown_signal()?;
        eprintln!("GitHub collector started");
        tokio::select! {
            biased;
            _ = shutdown => {}
            _ = self.run() => {}
        }
        eprintln!("GitHub collector stopped");
        Ok(())
    }

    async fn run(&mut self) {
        let mut schedule = tokio::time::interval(self.config.interval);
        schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            schedule.tick().await;
            self.cycle().await;
        }
    }

    pub async fn cycle(&mut self) {
        let ledger = self.config.ledger.join("/v0/facts/platform-syncs").unwrap();
        for (owner, config) in &self.config.owners {
            self.owners
                .entry(owner.clone())
                .or_default()
                .cycle(&self.client, &self.api, &ledger, owner, config)
                .await;
        }
    }
}

#[derive(Default)]
struct OwnerState {
    github: GitHub,
    heads: BTreeMap<String, CachedHead>,
    scopes: BTreeMap<String, ScopeState>,
}

struct CachedHead {
    node_id: String,
    pushed_at: Option<DateTime<Utc>>,
    head: Option<Head>,
}

impl CachedHead {
    fn is_current(&self, repo: &Repository) -> bool {
        self.node_id == repo.node_id && self.pushed_at == repo.pushed_at
    }
}

#[derive(Default)]
struct ScopeState {
    reported: BTreeMap<String, HeadIdentity>,
    pending: Option<Delivery>,
}

struct Delivery {
    snapshot: Snapshot,
    reported: BTreeMap<String, HeadIdentity>,
}

#[derive(Clone, PartialEq, Eq)]
struct HeadIdentity {
    node_id: String,
    branch: String,
    commit_sha: String,
}

impl OwnerState {
    async fn cycle(
        &mut self,
        client: &Client,
        api: &Url,
        ledger: &Url,
        owner: &str,
        config: &Owner,
    ) {
        let mut repositories = match self.github.list(client, api, owner, &config.token).await {
            Ok(repositories) => repositories,
            Err(error) => {
                eprintln!("GitHub read failed for owner {owner}: {error}");
                return;
            }
        };
        if !config.scopes.contains("*") {
            repositories.retain(|name, _| config.scopes.contains(name));
        }

        self.heads
            .retain(|name, _| repositories.get(name).is_some_and(|repo| !repo.archived));
        let candidates: Vec<_> = repositories
            .iter()
            .filter(|(name, repo)| {
                !repo.archived
                    && !self
                        .heads
                        .get(*name)
                        .is_some_and(|head| head.is_current(repo))
            })
            .collect();
        let observations = self
            .github
            .heads(client, api, &config.token, &candidates)
            .await;
        let mut failures = BTreeMap::new();
        for (name, result) in observations {
            let commit = match result {
                Ok(commit) => commit,
                Err(error) => {
                    failures.insert(name, error);
                    continue;
                }
            };
            let repo = &repositories[&name];
            let head = commit.map(|commit| {
                let sha = commit.oid.to_ascii_lowercase();
                if let Some(previous) = self
                    .heads
                    .get(&name)
                    .filter(|previous| previous.node_id == repo.node_id)
                    .and_then(|previous| previous.head.as_ref())
                    .filter(|head| head.branch == repo.default_branch && head.commit_sha == sha)
                {
                    return previous.clone();
                }
                Head {
                    name: repo.name.clone(),
                    branch: repo.default_branch.clone(),
                    commit_sha: sha,
                    committed_at: commit.committed_at,
                    observed_at: Utc::now(),
                }
            });
            self.heads.insert(
                name,
                CachedHead {
                    node_id: repo.node_id.clone(),
                    pushed_at: repo.pushed_at,
                    head,
                },
            );
        }

        let completed_at = Utc::now();
        for selector in &config.scopes {
            let failure = if selector == "*" {
                failures.values().next()
            } else {
                failures.get(selector)
            };
            if let Some(error) = failure {
                eprintln!(
                    "GitHub read failed for owner {owner}, scope {owner}/{selector}: {error}"
                );
                continue;
            }

            let state = self.scopes.entry(selector.clone()).or_default();
            // Retry the previous completed sync before advancing this scope's heartbeat.
            if let Some(pending) = state.pending.take()
                && !state.deliver(client, ledger, owner, pending).await
            {
                continue;
            }

            let entries: Vec<_> = if selector == "*" {
                repositories.iter().collect()
            } else {
                repositories.get_key_value(selector).into_iter().collect()
            };
            let mut reported = state.reported.clone();
            reported.retain(|name, _| {
                repositories.contains_key(name) && (selector == "*" || name == selector)
            });
            let mut heads = Vec::new();
            for (name, repo) in &entries {
                if let Some(head) = self
                    .heads
                    .get(*name)
                    .and_then(|cached| cached.head.as_ref())
                    .filter(|head| head.branch == repo.default_branch)
                {
                    let identity = HeadIdentity {
                        node_id: repo.node_id.clone(),
                        branch: head.branch.clone(),
                        commit_sha: head.commit_sha.clone(),
                    };
                    if reported.get(*name) != Some(&identity) {
                        reported.insert((*name).clone(), identity);
                        heads.push(Head {
                            name: repo.name.clone(),
                            ..head.clone()
                        });
                    }
                }
            }
            let snapshot = Snapshot {
                sender: "github-collector",
                platform: "github",
                host: "github.com",
                scope: format!("{owner}/{selector}"),
                completed_at,
                inventory: entries
                    .into_iter()
                    .map(|(_, repo)| InventoryEntry::from(repo))
                    .collect(),
                heads,
            };
            state
                .deliver(client, ledger, owner, Delivery { snapshot, reported })
                .await;
        }
    }
}

impl ScopeState {
    async fn deliver(
        &mut self,
        client: &Client,
        ledger: &Url,
        owner: &str,
        delivery: Delivery,
    ) -> bool {
        let response = client
            .post(ledger.clone())
            .json(&delivery.snapshot)
            .send()
            .await;
        let scope = &delivery.snapshot.scope;
        if let Ok(response) = response {
            let status = response.status();
            if status == StatusCode::NO_CONTENT {
                self.reported = delivery.reported;
                return true;
            }
            eprintln!(
                "Ledger delivery failed for owner {owner}, scope {scope}: HTTP {}",
                status.as_u16()
            );
            if matches!(
                status,
                StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE
            ) {
                // Rebuild a rejected snapshot on a later cycle as the inventory may change.
                return true;
            }
        } else {
            eprintln!("Ledger delivery failed for owner {owner}, scope {scope}: request failed");
        }
        self.pending = Some(delivery);
        false
    }
}

#[derive(Serialize)]
struct Snapshot {
    sender: &'static str,
    platform: &'static str,
    host: &'static str,
    scope: String,
    #[serde(serialize_with = "timestamp")]
    completed_at: DateTime<Utc>,
    inventory: Vec<InventoryEntry>,
    heads: Vec<Head>,
}

#[derive(Serialize)]
struct InventoryEntry {
    name: String,
    default_branch: String,
    archived: bool,
    fork: bool,
    last_activity_at: Option<String>,
}

impl From<&Repository> for InventoryEntry {
    fn from(repo: &Repository) -> Self {
        Self {
            name: repo.name.clone(),
            default_branch: repo.default_branch.clone(),
            archived: repo.archived,
            fork: repo.fork,
            last_activity_at: repo
                .pushed_at
                .map(|time| time.to_rfc3339_opts(SecondsFormat::Micros, true)),
        }
    }
}

#[derive(Clone, Serialize)]
struct Head {
    name: String,
    branch: String,
    commit_sha: String,
    #[serde(serialize_with = "timestamp")]
    committed_at: DateTime<Utc>,
    #[serde(serialize_with = "timestamp")]
    observed_at: DateTime<Utc>,
}

fn timestamp<S: Serializer>(time: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&time.to_rfc3339_opts(SecondsFormat::Micros, true))
}

#[cfg(unix)]
fn shutdown_signal() -> io::Result<impl Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
        }
    })
}

#[cfg(windows)]
fn shutdown_signal() -> io::Result<impl Future<Output = ()>> {
    let mut interrupt = tokio::signal::windows::ctrl_c()?;
    Ok(async move {
        interrupt.recv().await;
    })
}
