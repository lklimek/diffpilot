//! Job execution contract between server and runner: `JobSpec`, events, `JobResult`, the runner protocol and
//! the job state machine (brief §5 Job execution interface; `docs/contracts/runner-protocol.md`).

pub mod access;
pub mod event;
pub mod protocol;
pub mod result;
pub mod spec;
pub mod state;
