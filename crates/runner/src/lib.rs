//! Job runner: `execute(JobSpec, EventSink) -> JobResult`, the polling loop around it and the LLM providers.

pub mod client;
pub mod config;
pub mod execute;
pub mod git;
pub mod provider;
pub mod scrub;
pub mod sessions;
pub mod skills;
pub mod supervisor;
pub mod worker;

#[cfg(test)]
mod test_support;
