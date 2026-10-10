//! Protocol checks: the server binary over stdio, against a fake niri, a fake Noctalia and
//! fake programs in a directory of the test's own. Nothing here reaches the real desktop,
//! clipboard or audit log.
//!
//! Every module is `cfg(test)`, which always holds here, so Clippy treats the helpers as
//! test code too.

#[cfg(test)]
mod atspi;
#[cfg(test)]
mod client;
#[cfg(test)]
mod fixture;
#[cfg(test)]
mod niri;
#[cfg(test)]
mod noctalia;
#[cfg(test)]
#[path = "../../src/test_support/output_mode.rs"]
mod output_mode;

#[cfg(test)]
mod act;
#[cfg(test)]
mod audit;
#[cfg(test)]
mod cancellation;
#[cfg(test)]
mod control;
#[cfg(test)]
mod discovery;
#[cfg(test)]
mod elements;
#[cfg(test)]
mod engine;
#[cfg(test)]
mod errors;
#[cfg(test)]
mod events;
#[cfg(test)]
mod framing;
#[cfg(test)]
mod guard;
#[cfg(test)]
mod lease;
#[cfg(test)]
mod lifecycle;
#[cfg(test)]
mod recover;
#[cfg(test)]
mod screenshot;
#[cfg(test)]
mod session;
#[cfg(test)]
mod shared;
#[cfg(test)]
mod shell;
#[cfg(test)]
mod wait;
