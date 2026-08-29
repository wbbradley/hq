# Changelog

All notable changes to HQ are documented here. This project follows Semantic Versioning.

## [0.1.0] - Unreleased

First supported Rust release candidate. HQ has not shipped before this candidate, so there is no
upgrade or storage migration path.

### Added

- One native `hq` executable for Linux and macOS on x86-64 and ARM64.
- Signed causal domain state, SQLite persistence and repair, encrypted retained-relay sync, and
  exclusive local node ownership.
- Human, agent, mailbox, peer, relay, project, managed Codex, CLI, and terminal-UI workflows.
- Encrypted identity-only backup and isolated node-replacement rehearsal.
- Native release manifests, controlled relay/provider failure evidence, synthetic Go rollback
  rehearsal, and a cutover evidence bundle covering every acceptance and definition-of-done clause.

### Operational boundaries

- Codex CLI `0.150.1` is the pinned managed-provider baseline.
- Windows is not supported.
- Go keys, databases, protocols, commands, and service definitions are not imported or migrated.
- Soak and production cutover remain separate operator-authorized actions.
