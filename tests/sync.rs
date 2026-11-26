mod support;

use std::{
    collections::{HashMap, VecDeque},
    env::VarError,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::DateTime;
use github_collector::{Collector, Config};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use url::Url;

const TOKEN: &str = "synthetic-github-credential";
const BETA_TOKEN: &str = "synthetic-beta-credential";
const LEDGER: &str = "/v0/facts/platform-syncs";
const PUSH1: &str = "2026-11-01T10:00:00Z";
const PUSH2: &str = "2026-11-01T11:00:00Z";
const SHA1: &str = "1111111111111111111111111111111111111111";
const SHA2: &str = "2222222222222222222222222222222222222222";

struct Mock {
    url: Url,
    state: Shared,
    task: JoinHandle<()>,
}

type Shared = Arc<Mutex<Script>>;

#[derive(Default)]
struct Script {
    replies: VecDeque<Reply>,
    received: Vec<Received>,
}

struct Received {
    path: Uri,
    headers: HeaderMap,
    body: Value,
}

struct Reply {
    status: StatusCode,
    headers: Vec<(&'static str, String)>,
    body: Option<Value>,
}

impl Mock {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let state = Shared::default();
        let app = Router::new().fallback(receive).with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, state, task }
    }

    fn plan(&self, replies: impl IntoIterator<Item = Reply>) {
        let mut script = self.state.lock().unwrap();
        assert!(script.replies.is_empty());
        assert!(script.received.is_empty());
        script.replies.extend(replies);
    }

    fn take(&self, paths: &[&str]) -> Vec<Received> {
        let mut script = self.state.lock().unwrap();
        assert_eq!(script.replies.len(), 0, "unused mock replies");
        let received = std::mem::take(&mut script.received);
        assert_eq!(
            received.iter().map(|r| r.path.path()).collect::<Vec<_>>(),
            paths
        );
        for request in &received {
            if request.path.path() == LEDGER {
                assert!(!request.headers.contains_key("authorization"));
            } else {
                let authorization = request.headers["authorization"].to_str().unwrap();
                if request.path.path() == "/graphql" {
                    assert!(
                        authorization == format!("Bearer {TOKEN}")
                            || authorization == format!("Bearer {BETA_TOKEN}")
                    );
                } else {
                    let token = if request.path.path().split('/').any(|part| part == "beta") {
                        BETA_TOKEN
                    } else {
                        TOKEN
                    };
                    assert_eq!(authorization, format!("Bearer {token}"));
                }
            }
        }
        received
    }

    fn collector(&self, scopes: &str) -> Collector {
        let values = HashMap::from([
            ("GITHUB_COLLECTOR_SCOPES", scopes),
            ("GITHUB_COLLECTOR_LEDGER_URL", self.url.as_str()),
            ("GITHUB_COLLECTOR_TOKEN_ACME", TOKEN),
            ("GITHUB_COLLECTOR_TOKEN_BETA", BETA_TOKEN),
        ]);
        let config = Config::from_lookup(|key| {
            values
                .get(key)
                .map(|value| value.to_string())
                .ok_or(VarError::NotPresent)
        })
        .unwrap();
        Collector::with_github_url(config, self.url.clone()).unwrap()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn receive(State(state): State<Shared>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    let mut script = state.lock().unwrap();
    script.received.push(Received {
        path: parts.uri,
        headers: parts.headers,
        body,
    });
    let Some(reply) = script.replies.pop_front() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut response = match reply.body {
        Some(body) => (reply.status, Json(body)).into_response(),
        None => reply.status.into_response(),
    };
    for (key, value) in reply.headers {
        response.headers_mut().insert(key, value.parse().unwrap());
    }
    response
}

impl Reply {
    fn status(status: u16) -> Self {
        Self {
            status: StatusCode::from_u16(status).unwrap(),
            headers: vec![],
            body: None,
        }
    }

    fn ok(body: Value) -> Self {
        Self {
            body: Some(body),
            ..Self::status(200)
        }
    }

    fn header(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((key, value.into()));
        self
    }
}

fn login(owner: &str, kind: &str) -> Reply {
    Reply::ok(json!({"login": owner, "type": kind}))
}

fn repo(owner: &str, name: &str, pushed: Option<&str>, archived: bool) -> Value {
    json!({
        "node_id": format!("id-{name}"), "name": name, "owner": {"login": owner},
        "default_branch": "main", "fork": name == "fork", "archived": archived, "pushed_at": pushed,
    })
}

fn node(sha: &str) -> Value {
    json!({"defaultBranchRef": {"name": "main", "target": {"oid": sha, "committedDate": PUSH1}}})
}

fn graph(nodes: Value) -> Reply {
    Reply::ok(json!({"data": {"nodes": nodes}}))
}

fn assert_heads(payload: &Value, expected: &[(&str, &str)]) {
    let heads = payload["heads"].as_array().unwrap();
    let actual: Vec<_> = heads
        .iter()
        .map(|head| {
            assert_eq!(head["branch"], "main");
            assert_eq!(head["committed_at"], "2026-11-01T10:00:00.000000Z");
            let observed =
                DateTime::parse_from_rfc3339(head["observed_at"].as_str().unwrap()).unwrap();
            let completed =
                DateTime::parse_from_rfc3339(payload["completed_at"].as_str().unwrap()).unwrap();
            assert!(observed <= completed);
            (
                head["name"].as_str().unwrap(),
                head["commit_sha"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(actual, expected);
}

fn assert_batch(request: &Received, ids: &[&str]) {
    assert_eq!(request.body["variables"]["ids"], json!(ids));
    let query = request.body["query"].as_str().unwrap();
    assert!(query.contains("defaultBranchRef"));
    assert!(query.contains("oid committedDate"));
}

// REQ-0003 REQ-0004
#[tokio::test]
async fn cycles_cache_pages_and_publish_inventory_and_changed_heads() {
    const LIST: &str = "/orgs/acme/repos";
    let mock = Mock::start().await;
    let page2 = format!(
        "<{}orgs/acme/repos?per_page=100&page=2>; rel=\"next\"",
        mock.url
    );
    let mut collector = mock.collector("Acme/*,acme/gone");

    mock.plan([
        login("Acme", "Organization"),
        Reply::ok(json!([
            repo("acme", "active", Some(PUSH1), false),
            repo("acme", "fork", None, true)
        ]))
        .header("etag", "\"p1\"")
        .header("link", page2.clone()),
        Reply::ok(json!([repo("acme", "gone", Some(PUSH1), false)])).header("etag", "\"p2\""),
        graph(json!([node(SHA1), node(SHA1)])),
        Reply::status(204),
        Reply::status(204),
    ]);
    collector.cycle().await;
    let first = mock.take(&["/users/acme", LIST, LIST, "/graphql", LEDGER, LEDGER]);
    assert!(first[2].path.query().unwrap().contains("page=2"));
    assert_batch(&first[3], &["id-active", "id-gone"]);
    let payload = &first[4].body;
    assert_eq!(payload["scope"], "acme/*");
    assert_eq!(payload["sender"], "github-collector");
    assert_eq!(payload["platform"], "github");
    assert_eq!(payload["host"], "github.com");
    assert_eq!(
        payload["inventory"][0]["last_activity_at"],
        "2026-11-01T10:00:00.000000Z"
    );
    assert_eq!(
        payload["inventory"][1],
        json!({
            "name": "fork", "default_branch": "main", "archived": true, "fork": true, "last_activity_at": null
        })
    );
    assert_heads(payload, &[("active", SHA1), ("gone", SHA1)]);
    assert_eq!(first[5].body["inventory"], json!([payload["inventory"][2]]));
    assert_eq!(first[5].body["heads"], json!([payload["heads"][1]]));

    mock.plan([
        Reply::status(304),
        Reply::status(304),
        Reply::status(204),
        Reply::status(204),
    ]);
    collector.cycle().await;
    let unchanged = mock.take(&[LIST, LIST, LEDGER, LEDGER]);
    assert_eq!(unchanged[0].headers["if-none-match"], "\"p1\"");
    assert_eq!(unchanged[1].headers["if-none-match"], "\"p2\"");
    assert_eq!(unchanged[2].body["inventory"], payload["inventory"]);
    assert_heads(&unchanged[2].body, &[]);
    assert_heads(&unchanged[3].body, &[]);

    mock.plan([
        Reply::ok(json!([
            repo("acme", "active", Some(PUSH2), false),
            repo("acme", "fork", None, false),
            repo("acme", "empty", None, false),
        ]))
        .header("etag", "\"changed\""),
        graph(json!([node(SHA2), {"defaultBranchRef": null}, node(SHA1)])),
        Reply::status(204),
        Reply::status(204),
    ]);
    collector.cycle().await;
    let changed = mock.take(&[LIST, "/graphql", LEDGER, LEDGER]);
    assert_batch(&changed[1], &["id-active", "id-empty", "id-fork"]);
    assert_heads(&changed[2].body, &[("active", SHA2), ("fork", SHA1)]);
    assert_eq!(changed[2].body["inventory"].as_array().unwrap().len(), 3);
    assert_eq!(changed[2].body["inventory"][2]["archived"], false);
    assert_eq!(changed[3].body["inventory"], json!([]));
    assert_heads(&changed[3].body, &[]);

    mock.plan([
        Reply::ok(json!([
            repo("acme", "active", Some(PUSH1), false),
            repo("acme", "fork", None, false),
            repo("acme", "empty", None, false),
        ])),
        graph(json!([node(SHA2)])),
        Reply::status(204),
        Reply::status(204),
    ]);
    collector.cycle().await;
    let same_head = mock.take(&[LIST, "/graphql", LEDGER, LEDGER]);
    assert_batch(&same_head[1], &["id-active"]);
    assert_heads(&same_head[2].body, &[]);
}

// REQ-0003 REQ-0004
#[tokio::test]
async fn checks_past_a_304_on_a_full_terminal_page_and_rebatches_after_restart() {
    const LIST: &str = "/orgs/acme/repos";
    let mock = Mock::start().await;
    let mut collector = mock.collector("acme/*");
    let first_page = (0..100)
        .map(|index| repo("acme", &format!("repo-{index:03}"), Some(PUSH1), false))
        .collect::<Vec<_>>();
    let page2 =
        Reply::ok(json!([repo("acme", "repo-100", Some(PUSH2), false)])).header("etag", "\"p2\"");
    let first_nodes = graph(json!(vec![node(SHA1); 100]));
    mock.plan([
        login("acme", "Organization"),
        Reply::ok(json!(first_page)).header("etag", "\"p1\""),
        first_nodes,
        Reply::status(204),
    ]);
    collector.cycle().await;
    let initial = mock.take(&["/users/acme", LIST, "/graphql", LEDGER]);
    assert_eq!(
        initial[2].body["variables"]["ids"]
            .as_array()
            .unwrap()
            .len(),
        100
    );

    mock.plan([
        Reply::status(304),
        page2,
        graph(json!([node(SHA2)])),
        Reply::status(204),
    ]);
    collector.cycle().await;
    let appended = mock.take(&[LIST, LIST, "/graphql", LEDGER]);
    assert!(appended[1].path.query().unwrap().contains("page=2"));
    assert_batch(&appended[2], &["id-repo-100"]);
    assert_eq!(appended[3].body["inventory"].as_array().unwrap().len(), 101);
    assert_heads(&appended[3].body, &[("repo-100", SHA2)]);

    let mut restarted = mock.collector("acme/*");
    let link = format!(
        "<{}orgs/acme/repos?per_page=100&page=2>; rel=\"next\"",
        mock.url
    );
    mock.plan([
        login("acme", "Organization"),
        Reply::ok(json!(first_page)).header("link", link),
        Reply::ok(json!([repo("acme", "repo-100", Some(PUSH2), false)])),
        graph(json!(vec![node(SHA1); 100])),
        graph(json!([node(SHA2)])),
        Reply::status(204),
    ]);
    restarted.cycle().await;
    let restarted = mock.take(&["/users/acme", LIST, LIST, "/graphql", "/graphql", LEDGER]);
    assert!(!restarted[1].headers.contains_key("if-none-match"));
    assert_eq!(
        restarted[3].body["variables"]["ids"]
            .as_array()
            .unwrap()
            .len(),
        100
    );
    assert_batch(&restarted[4], &["id-repo-100"]);
    assert_eq!(restarted[5].body["heads"].as_array().unwrap().len(), 101);
}

// REQ-0003 REQ-0004
#[tokio::test]
async fn failed_reads_omit_only_affected_scopes_and_recover_next_cycle() {
    let mock = Mock::start().await;
    let mut collector = mock.collector("acme/*,acme/a,acme/b,beta/*");
    let list = || {
        Reply::ok(json!([
            repo("acme", "a", Some(PUSH1), false),
            repo("acme", "b", Some(PUSH1), false),
            repo("acme", "outside", Some(PUSH1), true),
        ]))
        .header("etag", "\"repos\"")
    };
    mock.plan([
        login("acme", "Organization"),
        list(),
        Reply::ok(json!({
            "data": {"nodes": [null, node(SHA1)]},
            "errors": [{"message": TOKEN, "path": ["nodes", 0]}],
        })),
        Reply::status(204),
        login("beta", "Organization"),
        Reply::status(502),
    ]);
    collector.cycle().await;
    let first = mock.take(&[
        "/users/acme",
        "/orgs/acme/repos",
        "/graphql",
        LEDGER,
        "/users/beta",
        "/orgs/beta/repos",
    ]);
    assert_eq!(first[3].body["scope"], "acme/b");
    assert_heads(&first[3].body, &[("b", SHA1)]);

    mock.plan([
        Reply::status(304),
        graph(json!([node(SHA2)])),
        Reply::status(204),
        Reply::status(204),
        Reply::status(204),
        Reply::ok(json!([])),
        Reply::status(204),
    ]);
    collector.cycle().await;
    let recovered = mock.take(&[
        "/orgs/acme/repos",
        "/graphql",
        LEDGER,
        LEDGER,
        LEDGER,
        "/orgs/beta/repos",
        LEDGER,
    ]);
    assert_batch(&recovered[1], &["id-a"]);
    assert_heads(&recovered[2].body, &[("a", SHA2), ("b", SHA1)]);
    assert_eq!(recovered[2].body["heads"][1], first[3].body["heads"][0]);
    assert_eq!(recovered[3].body["scope"], "acme/a");
    assert_heads(&recovered[4].body, &[]);
    assert_eq!(recovered[6].body["scope"], "beta/*");
}

// REQ-0003 REQ-0004
#[tokio::test]
async fn personal_inventory_uses_owner_credentials_and_retries_original_snapshot() {
    let mock = Mock::start().await;
    let mut collector = mock.collector("Acme/repo,acme/REPO");
    mock.plan([
        login("acme", "User"),
        login("acme", "User"),
        Reply::ok(json!([
            repo("acme", "elsewhere", Some(PUSH1), false),
            repo("acme", "Repo", Some(PUSH1), false)
        ]))
        .header("etag", "\"owned\""),
        graph(json!([node(SHA1)])),
        Reply::status(500),
    ]);
    collector.cycle().await;
    let first = mock.take(&["/users/acme", "/user", "/user/repos", "/graphql", LEDGER]);
    assert!(first[2].path.query().unwrap().contains("affiliation=owner"));
    assert_batch(&first[3], &["id-Repo"]);
    assert_eq!(first[4].body["inventory"].as_array().unwrap().len(), 1);
    assert_heads(&first[4].body, &[("Repo", SHA1)]);

    mock.plan([Reply::status(304), Reply::status(204), Reply::status(204)]);
    collector.cycle().await;
    let retried = mock.take(&["/user/repos", LEDGER, LEDGER]);
    assert_eq!(retried[1].body, first[4].body);
    assert_heads(&retried[2].body, &[]);
    assert_ne!(
        retried[2].body["completed_at"],
        first[4].body["completed_at"]
    );
}

// REQ-0005
#[tokio::test]
async fn retry_after_and_graphql_primary_reset_are_respected_across_cycles() {
    let mock = Mock::start().await;
    let mut collector = mock.collector("acme/*,beta/*");
    let reset = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 120;
    mock.plan([
        login("acme", "Organization"),
        Reply::status(429).header("retry-after", "120"),
        login("beta", "Organization"),
        Reply::ok(json!([repo("beta", "repo", Some(PUSH1), false)])).header("etag", "\"beta\""),
        Reply::ok(json!({"errors": [{"type": "RATE_LIMITED", "message": TOKEN}]}))
            .header("x-ratelimit-remaining", "0")
            .header("x-ratelimit-reset", reset.to_string()),
    ]);
    collector.cycle().await;
    mock.take(&[
        "/users/acme",
        "/orgs/acme/repos",
        "/users/beta",
        "/orgs/beta/repos",
        "/graphql",
    ]);

    mock.plan([Reply::status(304)]);
    collector.cycle().await;
    let remaining = mock.take(&["/orgs/beta/repos"]);
    assert_eq!(remaining[0].headers["if-none-match"], "\"beta\"");
}

// REQ-0005
#[tokio::test]
async fn platform_and_ledger_error_logs_omit_response_content_and_credentials() {
    let mock = Mock::start().await;
    mock.plan([
        login("acme", "Organization"),
        Reply::ok(json!([])),
        Reply {
            body: Some(json!({"message": TOKEN})),
            ..Reply::status(500)
        },
        login("beta", "Organization"),
        Reply {
            body: Some(json!({"message": TOKEN})),
            ..Reply::status(403)
        },
    ]);
    let endpoint = mock.url.to_string();
    let child = tokio::task::spawn_blocking(move || {
        support::command(std::env::current_exe().unwrap())
            .args(["--exact", "cycle_child", "--nocapture"])
            .env("GITHUB_COLLECTOR_TEST_API", &endpoint)
            .env("GITHUB_COLLECTOR_LEDGER_URL", &endpoint)
            .env("GITHUB_COLLECTOR_SCOPES", "acme/*,beta/*")
            .env("GITHUB_COLLECTOR_TOKEN_ACME", TOKEN)
            .env("GITHUB_COLLECTOR_TOKEN_BETA", BETA_TOKEN)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(child.status.success());
    let stderr = String::from_utf8(child.stderr).unwrap();
    assert!(stderr.contains("Ledger delivery failed for owner acme"));
    assert!(stderr.contains("GitHub read failed for owner beta"));
    assert!(!stderr.contains(TOKEN));
    assert!(!stderr.contains(BETA_TOKEN));
    assert!(!String::from_utf8(child.stdout).unwrap().contains(TOKEN));
    mock.take(&[
        "/users/acme",
        "/orgs/acme/repos",
        LEDGER,
        "/users/beta",
        "/orgs/beta/repos",
    ]);
}

#[tokio::test]
async fn cycle_child() {
    let Ok(endpoint) = std::env::var("GITHUB_COLLECTOR_TEST_API") else {
        return;
    };
    let config = Config::from_env().unwrap();
    let mut collector = Collector::with_github_url(config, endpoint.parse().unwrap()).unwrap();
    collector.cycle().await;
}
