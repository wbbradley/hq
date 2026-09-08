# Continuous integration

Every push to `main`, pull request, and manual CI dispatch runs `.github/workflows/ci.yml` without path filters.

- Go tests, vet, and build run on Linux and macOS; Linux also runs every Go package with the race detector. Race instrumentation makes the cryptographic store suite substantially slower, so it has an explicit 25-minute package timeout within a 30-minute job.
- All Rust workspace packages run tests with all targets and features on Linux and macOS. Linux enumerates workspace membership using Cargo metadata; adding a crate automatically adds its test job. Separate doctest steps cover documentation examples, which `--all-targets` excludes.
- Every `scripts/test-*.sh` contract test runs on Linux and macOS, alongside formatting, strict Clippy, architecture, specification, and normal-operation independence checks.
- CI also runs portable target checks, four native qualification platforms using optimized builds and evidence aggregation, protocol fuzz smoke, and dependency policy checks.

Two explicitly ignored tests require external services and remain opt-in: `hq-codex`'s `installed_codex_smoke` requires authenticated Codex 0.150.1 and `HQ_CODEX_INSTALLED_SMOKE=1`; `hq-relay`'s `rnostr_interop` requires a controlled relay and `HQ_RUN_CONTROLLED_RELAY_SMOKE=1`. Ordinary CI exercises the self-contained adapter and installed-process fixtures. These two live smoke tests are not claimed as part of push CI coverage.
