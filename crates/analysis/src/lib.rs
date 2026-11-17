//! Deterministic, language-agnostic analysis of a pull request; runs `git` read-only on a given path.

pub mod blob;
pub mod counters;
pub mod coverage;
pub mod diff;
pub mod lines;
pub mod moves;
pub mod order;
pub mod prepass;
pub mod related;
pub mod tech;
pub mod verify;

pub mod tagged;
