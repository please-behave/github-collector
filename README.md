# GitHub Collector

[![build](https://github.com/please-behave/github-collector/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/please-behave/github-collector/actions/workflows/ci.yml)

The GitHub collector supplies repository inventory and default-branch observations to the Please Behave ledger. See the [shared Why](https://github.com/please-behave/why) for the product's purpose and design.

The current executable validates its environment and waits for a shutdown signal. Its startup configuration is defined in the [collector requirements](why/README.md).
