# Why

This directory records the GitHub collector's requirements and decisions. The [shared Why](https://github.com/please-behave/why) defines the product principles and the [artifact standard](https://github.com/please-behave/why/tree/main/standard).

## Artifacts

- [REQ-0001](requirements/REQ-0001.yaml) Environment configuration
- [REQ-0002](requirements/REQ-0002.yaml) Graceful shutdown
- [REQ-0003](requirements/REQ-0003.yaml) Collect repository inventory and sampled heads
- [REQ-0004](requirements/REQ-0004.yaml) Deliver completed platform syncs
- [REQ-0005](requirements/REQ-0005.yaml) Respect GitHub rate limits
- [COMP-0001](components/COMP-0001.yaml) Environment Configuration
- [COMP-0002](components/COMP-0002.yaml) GitHub Access
- [COMP-0003](components/COMP-0003.yaml) Collection Lifecycle and Delivery

Components partition the compiled code below `src`. Requirements carry one `area`: `operations` for startup and shutdown, `collection` for reading GitHub and delivering syncs.

The collector follows the shared [Repository Inventory and Status decision](https://github.com/please-behave/why/blob/main/why/decisions/DEC-0024.yaml). The [ledger](https://github.com/please-behave/ledger) owns the reporting contract and stores the collected facts. [Repository aliases](repositories.yaml) identify the sources of cross-repository references.

## GitHub API behavior

The authenticated REST repository listings expose `pushed_at`, corresponding
to the GitHub repository's most recent push. Pushes to other branches can
trigger a head read with the same result. A change that leaves `pushed_at`
unchanged, such as changing the default branch setting without a push, may
go unobserved until another push or a restart.
[REST repository listings](https://docs.github.com/en/rest/repos/repos)
[GraphQL repository fields](https://docs.github.com/en/graphql/reference/repos)

GitHub documents that a correctly authenticated conditional GET returning
`304 Not Modified` uses no primary REST quota. The collector reuses the
cached page and its pagination when GitHub omits the response body and
pagination header. It checks beyond a full cached terminal page to catch new
repositories even if that page's body remains the same.
[GitHub conditional requests](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api#use-conditional-requests)
[GitHub pagination](https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api)

Organization inventory uses the authenticated organization repository listing.
Personal inventory uses the authenticated account's owned-repository listing,
which can include private repositories. The credential must have read access
to every repository in scope, including the contents needed to read the Git
refs and commits. GitHub only returns repositories visible to that credential.
[GitHub repository listings](https://docs.github.com/en/rest/repos/repos)
