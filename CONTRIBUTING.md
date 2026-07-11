# Contributing to Pegasus

## License

Pegasus is licensed under **GPL-3.0-or-later** (see [LICENSE](LICENSE)).

Contributions are accepted under the terms of the **Contributor License
Agreement** in [CLA.md](CLA.md) — please read it before opening a pull
request. In short: your Contribution stays GPL-3.0-or-later for everyone,
but you also grant the Maintainer the right to relicense it (e.g. to offer
a separate commercial license to a company that wants a closed-source
build). You keep full ownership and copyright of your own work.

First-time contributors: add a line to your pull request description
confirming you agree to the CLA, e.g.:

> I have read and agree to the Pegasus CLA (CLA.md).

## Making changes

See [CLAUDE.md](CLAUDE.md) for the architecture overview, build/test
commands, and the project's conventions (determinism rules for `src/sim.rs`,
the deploy pipeline, etc.). Run `cargo build` and `cargo test` before
opening a pull request.
