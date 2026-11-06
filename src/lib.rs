//! omarchy-oc: overclocking and tuning for an Omarchy desktop.
//!
//! * `omarchy-ocd` — root daemon: sampling, profiles, safety (confirm/revert,
//!   crash guard, watchdog), auto-switch rules, metrics, control socket
//! * `omarchy-oc` — CLI, also used by the shell plugin; runs stress tests
//! * `qqq.oc` — Omarchy shell plugin (bar widget + panel)

pub mod apply;
pub mod config;
pub mod cpu;
pub mod daemon;
pub mod fans;
pub mod gpu;
pub mod health;
pub mod history;
pub mod mem;
pub mod metrics;
pub mod platform;
pub mod smu;
pub mod stress;
pub mod tune;
pub mod util;
