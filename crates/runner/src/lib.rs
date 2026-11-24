//! Job runner: `execute(JobSpec, EventSink) -> JobResult`, the polling loop around it and the LLM providers.

pub mod client;
pub mod config;
pub mod execute;
pub mod git;
pub mod launcher;
pub mod phases;
pub mod provider;
pub mod scratch;
pub mod scrub;
pub mod worker;

#[cfg(test)]
mod test_support;
