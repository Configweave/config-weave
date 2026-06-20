//! `config-weave test`: run package convergence tests inside disposable
//! backend instances — docker containers (linux) or vmlab VMs (linux or
//! windows). The runner copies a static config-weave binary into the
//! instance, synthesizes a minimal playbook for the package under test,
//! drives check/apply through the real engine via `--json`, and
//! evaluates per-step expectations from the parsed reports.

pub mod backend;
pub mod docker;
pub mod output;
pub mod report;
pub mod runner;
pub mod synth;
pub mod vmlab;
