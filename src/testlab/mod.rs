//! `config-weave test`: run package convergence tests inside disposable
//! vmlab instances — a container built from an OCI image (linux) or a VM
//! cloned from a template (linux or windows). The runner copies a static
//! config-weave binary into the instance, synthesizes a minimal playbook
//! for the package under test, drives check/apply through the real engine
//! via `--json`, and evaluates per-step expectations from the parsed
//! reports.

pub mod backend;
pub mod events;
pub mod output;
pub mod report;
pub mod runner;
pub mod synth;
pub mod vmlab;
