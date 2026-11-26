use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::{Duration, SystemTime},
};

use chrono::{DateTime, Utc};
use reqwest::{
    Client, RequestBuilder, Response, StatusCode,
    header::{
        ACCEPT, AUTHORIZATION, ETAG, HeaderMap, HeaderValue, IF_NONE_MATCH, LINK, RETRY_AFTER,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use crate::config::valid_name;

const PAGE_SIZE: usize = 100;
const HEAD_BATCH_SIZE: usize = 100;
const HEAD_QUERY: &str = r"
    query DefaultBranchHeads($ids: [ID!]!) {
      nodes(ids: $ids) {
        ... on Repository {
          defaultBranchRef {
            name
            target { ... on Commit { oid committedDate } }
          }
        }
      }
    }
";

#[derive(Clone, Deserialize)]
pub(crate) struct Repository {
    pub node_id: String,
    pub name: String,
    pub default_branch: String,
    pub archived: bool,
    pub fork: bool,
    pub pushed_at: Option<DateTime<Utc>>,
    owner: Login,
}

#[derive(Clone, Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
struct Account {
    login: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Clone)]
struct Page {
    etag: Option<HeaderValue>,
    repositories: Vec<Repository>,
    next: Option<Url>,
}

#[derive(Default)]
pub(crate) struct GitHub {
    list_url: Option<Url>,
    pages: HashMap<Url, Page>,
    limits: Limits,
}

#[derive(Clone, Deserialize)]
pub(crate) struct Commit {
    pub oid: String,
    #[serde(rename = "committedDate")]
    pub committed_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct Branch {
    name: String,
    target: Commit,
}

#[derive(Deserialize)]
struct Node {
    // The key must be present. An explicit null means GitHub has no default ref.
    #[serde(
        rename = "defaultBranchRef",
        deserialize_with = "Deserialize::deserialize"
    )]
    branch: Option<Branch>,
}

#[derive(Deserialize)]
struct GraphQl {
    data: Option<Nodes>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Deserialize)]
struct Nodes {
    nodes: Option<Vec<Option<Value>>>,
}

#[derive(Deserialize)]
struct GraphQlError {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    path: Vec<Value>,
}

pub(crate) type ReadResult<T> = Result<T, &'static str>;

impl GitHub {
    pub async fn list(
        &mut self,
        client: &Client,
        api: &Url,
        owner: &str,
        token: &HeaderValue,
    ) -> ReadResult<BTreeMap<String, Repository>> {
        if self.list_url.is_none() {
            self.list_url = Some(self.list_url(client, api, owner, token).await?);
        }
        let first = self.list_url.as_ref().unwrap();
        let mut url = first.clone();
        let mut page_number = 1;
        let mut pages = HashMap::new();
        let mut repositories = BTreeMap::new();
        loop {
            if pages.contains_key(&url) {
                return Err("GitHub returned a pagination loop");
            }
            let cached = self.pages.get(&url);
            let mut request = client.get(url.clone());
            if let Some(etag) = cached.and_then(|page| page.etag.as_ref()) {
                request = request.header(IF_NONE_MATCH, etag);
            }
            let response = self.limits.send(request, token, Api::Rest).await?;
            let next = next_link(response.headers(), api)?;

            let page = if response.status() == StatusCode::NOT_MODIFIED {
                let mut page = cached.cloned().ok_or("GitHub returned an uncached 304")?;
                // A 304 may omit Link. Probe past a full terminal page in case
                // a repository was appended without changing that page's body.
                page.next = next.or(page.next);
                if page.next.is_none() && page.repositories.len() == PAGE_SIZE {
                    page.next = Some(numbered_page(first, page_number + 1));
                }
                page
            } else {
                Page {
                    next,
                    etag: response.headers().get(ETAG).cloned(),
                    repositories: response
                        .json()
                        .await
                        .map_err(|_| "invalid GitHub repository page")?,
                }
            };

            for repository in &page.repositories {
                if !repository.owner.login.eq_ignore_ascii_case(owner)
                    || !valid_name(&repository.name)
                    || !valid_branch(&repository.default_branch)
                    || repository.node_id.is_empty()
                {
                    return Err("GitHub returned invalid repository metadata");
                }
                if repositories
                    .insert(repository.name.to_ascii_lowercase(), repository.clone())
                    .is_some()
                {
                    return Err("GitHub returned duplicate repositories across pages");
                }
            }
            let next = page.next.clone();
            pages.insert(url, page);
            let Some(next) = next else { break };
            url = next;
            page_number += 1;
        }
        self.pages = pages;
        Ok(repositories)
    }

    async fn list_url(
        &mut self,
        client: &Client,
        api: &Url,
        owner: &str,
        token: &HeaderValue,
    ) -> ReadResult<Url> {
        let account_url = api.join(&format!("users/{owner}")).unwrap();
        let account: Account = self
            .limits
            .send(client.get(account_url), token, Api::Rest)
            .await?
            .json()
            .await
            .map_err(|_| "invalid GitHub owner response")?;
        if !account.login.eq_ignore_ascii_case(owner) {
            return Err("GitHub returned a different owner");
        }

        let mut url = match account.kind.as_str() {
            "Organization" => {
                let mut url = api.join(&format!("orgs/{owner}/repos")).unwrap();
                url.query_pairs_mut().append_pair("type", "all");
                url
            }
            "User" => {
                let viewer: Login = self
                    .limits
                    .send(client.get(api.join("user").unwrap()), token, Api::Rest)
                    .await?
                    .json()
                    .await
                    .map_err(|_| "invalid GitHub credential identity response")?;
                if !viewer.login.eq_ignore_ascii_case(owner) {
                    return Err("GitHub credential must belong to the configured personal owner");
                }
                let mut url = api.join("user/repos").unwrap();
                url.query_pairs_mut().append_pair("affiliation", "owner");
                url
            }
            _ => return Err("unsupported GitHub owner type"),
        };
        url.query_pairs_mut()
            .append_pair("per_page", &PAGE_SIZE.to_string())
            .append_pair("sort", "created")
            .append_pair("direction", "asc");
        Ok(url)
    }

    pub async fn heads(
        &mut self,
        client: &Client,
        api: &Url,
        token: &HeaderValue,
        repositories: &[(&String, &Repository)],
    ) -> BTreeMap<String, ReadResult<Option<Commit>>> {
        let mut heads = BTreeMap::new();
        for batch in repositories.chunks(HEAD_BATCH_SIZE) {
            let results = self.head_batch(client, api, token, batch).await;
            match results {
                Ok(results) => {
                    for ((name, _), result) in batch.iter().zip(results) {
                        heads.insert((*name).clone(), result);
                    }
                }
                Err(error) => {
                    for (name, _) in batch {
                        heads.insert((*name).clone(), Err(error));
                    }
                }
            }
        }
        heads
    }

    async fn head_batch(
        &mut self,
        client: &Client,
        api: &Url,
        token: &HeaderValue,
        batch: &[(&String, &Repository)],
    ) -> ReadResult<Vec<ReadResult<Option<Commit>>>> {
        let ids: Vec<_> = batch.iter().map(|(_, repo)| &repo.node_id).collect();
        let request = client
            .post(api.join("graphql").unwrap())
            .json(&json!({ "query": HEAD_QUERY, "variables": { "ids": ids } }));
        let response = self.limits.send(request, token, Api::GraphQl).await?;
        let retry_after = retry_deadline(response.headers(), SystemTime::now());
        let graph: GraphQl = response
            .json()
            .await
            .map_err(|_| "invalid GitHub GraphQL response")?;
        if graph
            .errors
            .iter()
            .any(|error| error.kind == "RATE_LIMITED" || is_rate_error(&error.message))
        {
            if retry_after.is_some() || !self.limits.primary_active(Api::GraphQl) {
                self.limits.pause_secondary(Api::GraphQl, retry_after);
            }
            return Err("GitHub rate limited the GraphQL request");
        }
        self.limits.clear_secondary_history(Api::GraphQl);

        let mut failed = HashSet::new();
        for error in &graph.errors {
            let [field, index, ..] = error.path.as_slice() else {
                return Err("GitHub GraphQL query failed");
            };
            if field != "nodes" {
                return Err("GitHub GraphQL query failed");
            }
            let index = index
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .filter(|&index| index < batch.len())
                .ok_or("invalid GitHub GraphQL error path")?;
            failed.insert(index);
        }
        let nodes = graph
            .data
            .and_then(|data| data.nodes)
            .filter(|nodes| nodes.len() == batch.len())
            .ok_or("GitHub did not return the requested head batch")?;
        Ok(nodes
            .into_iter()
            .zip(batch)
            .enumerate()
            .map(|(index, (node, (_, repo)))| {
                if failed.contains(&index) {
                    return Err("GitHub GraphQL head read failed");
                }
                let node: Node =
                    serde_json::from_value(node.ok_or("GitHub omitted a repository node")?)
                        .map_err(|_| "invalid GitHub default branch response")?;
                let Some(branch) = node.branch else {
                    return Ok(None);
                };
                if branch.name != repo.default_branch
                    || !matches!(branch.target.oid.len(), 40 | 64)
                    || !branch
                        .target
                        .oid
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err("GitHub returned an inconsistent default branch head");
                }
                Ok(Some(branch.target))
            })
            .collect())
    }
}

fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && branch.chars().count() <= 255
        && !branch
            .chars()
            .any(|character| character.is_whitespace() || character == '\0')
}

fn next_link(headers: &HeaderMap, api: &Url) -> ReadResult<Option<Url>> {
    for value in headers.get_all(LINK) {
        let value = value
            .to_str()
            .map_err(|_| "invalid GitHub pagination header")?;
        for link in value.split(',') {
            let Some((target, parameters)) = link.trim().split_once('>') else {
                return Err("invalid GitHub pagination header");
            };
            if parameters
                .split(';')
                .any(|parameter| parameter.trim() == "rel=\"next\"")
            {
                let url = target
                    .strip_prefix('<')
                    .and_then(|target| api.join(target).ok())
                    .filter(|url| {
                        url.origin() == api.origin()
                            && url.username().is_empty()
                            && url.password().is_none()
                            && url.fragment().is_none()
                    })
                    .ok_or("invalid GitHub pagination destination")?;
                return Ok(Some(url));
            }
        }
    }
    Ok(None)
}

fn numbered_page(first: &Url, page: usize) -> Url {
    let mut url = first.clone();
    url.query_pairs_mut().append_pair("page", &page.to_string());
    url
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Api {
    Rest,
    GraphQl,
}

#[derive(Default)]
struct Limits {
    rest: Option<SystemTime>,
    graphql: Option<SystemTime>,
    secondary: Option<(Api, SystemTime)>,
    secondary_failures: u8,
}

impl Limits {
    fn primary_active(&self, api: Api) -> bool {
        match api {
            Api::Rest => self.rest,
            Api::GraphQl => self.graphql,
        }
        .is_some_and(|until| until > SystemTime::now())
    }

    fn pause_secondary(&mut self, api: Api, retry: Option<SystemTime>) {
        let fallback = Duration::from_secs(60 * (1 << self.secondary_failures));
        self.secondary = retry
            .or_else(|| SystemTime::now().checked_add(fallback))
            .map(|until| (api, until));
        self.secondary_failures = (self.secondary_failures + 1).min(6);
    }

    fn clear_secondary_history(&mut self, api: Api) {
        if self
            .secondary
            .is_some_and(|(penalized, _)| penalized == api)
        {
            self.secondary = None;
            self.secondary_failures = 0;
        }
    }

    async fn send(
        &mut self,
        request: RequestBuilder,
        token: &HeaderValue,
        api: Api,
    ) -> ReadResult<Response> {
        let now = SystemTime::now();
        if self.primary_active(api) || self.secondary.is_some_and(|(_, until)| until > now) {
            return Err("GitHub rate-limit backoff is active");
        }
        let response = request
            .header(AUTHORIZATION, token)
            .header(ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|_| "GitHub request failed")?;
        let status = response.status();
        let headers = response.headers();
        let now = SystemTime::now();
        let exhausted = headers
            .get("x-ratelimit-remaining")
            .is_some_and(|value| value == "0");
        if exhausted {
            let reset = headers
                .get("x-ratelimit-reset")
                .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
                .and_then(|seconds| {
                    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
                })
                .filter(|&reset| reset > now)
                .or_else(|| now.checked_add(Duration::from_secs(60)));
            *match api {
                Api::Rest => &mut self.rest,
                Api::GraphQl => &mut self.graphql,
            } = reset;
        }

        if status == StatusCode::OK
            || (matches!(api, Api::Rest) && status == StatusCode::NOT_MODIFIED)
        {
            if api == Api::Rest {
                self.clear_secondary_history(api);
            }
            return Ok(response);
        }

        let retry = retry_deadline(headers, now);
        let secondary = retry.is_some()
            || status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN
                && !exhausted
                && response.json::<Value>().await.ok().is_some_and(|body| {
                    let rate_message = |value: &Value| {
                        value
                            .get("message")
                            .and_then(Value::as_str)
                            .is_some_and(is_rate_error)
                    };
                    rate_message(&body)
                        || body
                            .get("errors")
                            .and_then(Value::as_array)
                            .is_some_and(|errors| errors.iter().any(rate_message))
                }));
        if secondary {
            if retry.is_some() || !exhausted {
                self.pause_secondary(api, retry);
            }
            return Err("GitHub rate limited the request");
        }
        if exhausted {
            return Err("GitHub primary rate limit is exhausted");
        }
        Err(match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "GitHub denied access",
            StatusCode::NOT_FOUND => "GitHub could not find the requested resource",
            _ => "GitHub returned an unsuccessful response",
        })
    }
}

fn is_rate_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("rate limit") || message.contains("abuse detection")
}

fn retry_deadline(headers: &HeaderMap, now: SystemTime) -> Option<SystemTime> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return now.checked_add(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value).ok()
}
