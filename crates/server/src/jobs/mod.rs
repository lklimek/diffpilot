//! Job execution: the `JobExecutor` seam, the database queue (`QueueExecutor`), job claiming and the progress
//! channels behind SSE.

pub mod claim;
pub mod executor;
pub mod progress;
pub mod queue;
