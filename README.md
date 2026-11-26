# GitHub Collector

[![build](https://github.com/please-behave/github-collector/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/please-behave/github-collector/actions/workflows/ci.yml) [![release](https://img.shields.io/github/v/release/please-behave/github-collector?display_name=tag&sort=semver)](https://github.com/please-behave/github-collector/releases) [![license](https://img.shields.io/badge/license-PolyForm%20Noncommercial%201.0.0-3949ab.svg)](https://polyformproject.org/licenses/noncommercial/1.0.0/)

[![tests](https://christianpflugradt.github.io/badges-and-stuff/please-behave-github-collector/tests.svg)](https://christianpflugradt.github.io/badges-and-stuff/) [![coverage](https://christianpflugradt.github.io/badges-and-stuff/please-behave-github-collector/coverage.svg)](https://christianpflugradt.github.io/badges-and-stuff/) [![LOC](https://christianpflugradt.github.io/badges-and-stuff/please-behave-github-collector/loc.svg)](https://christianpflugradt.github.io/badges-and-stuff/)

The GitHub collector supplies repository inventory and default-branch observations to the Please Behave ledger. See the [shared Why](https://github.com/please-behave/why) for the product's purpose and design.

The collector runs periodic GitHub syncs and sends each completed scope to
the ledger's internal platform-sync endpoint. See the [collector requirements](why/README.md)
for its configuration and GitHub API behavior.
