# GitHub Collector

[![build](https://github.com/please-behave/github-collector/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/please-behave/github-collector/actions/workflows/ci.yml)

The GitHub collector supplies repository inventory and default-branch observations to the Please Behave ledger. See the [shared Why](https://github.com/please-behave/why) for the product's purpose and design.

The collector runs periodic GitHub syncs and sends each completed scope to
the ledger's internal platform-sync endpoint. See the [collector requirements](why/README.md)
for its configuration and GitHub API behavior.
