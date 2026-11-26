# Why

This directory records the GitHub collector's requirements and decisions. The [shared Why](https://github.com/please-behave/why) defines the product principles and the [artifact standard](https://github.com/please-behave/why/tree/main/standard).

## Artifacts

- [REQ-0001](requirements/REQ-0001.yaml) Environment configuration
- [REQ-0002](requirements/REQ-0002.yaml) Graceful shutdown

The collector follows the shared [Repository Inventory and Status decision](https://github.com/please-behave/why/blob/main/why/decisions/DEC-0024.yaml). The [ledger](https://github.com/please-behave/ledger) owns the reporting contract and stores the collected facts. [Repository aliases](repositories.yaml) identify the sources of cross-repository references.
