//! Identifiers. Database-backed entities use UUIDs; analysis content uses short strings stable within one
//! analysis (plan §5.3).

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
        )]
        pub struct $name(pub Uuid);

        impl $name {
            /// A new random identifier.
            pub fn new_random() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
        )]
        pub struct $name(pub String);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

uuid_id!(
    /// Identifier of an analysis (one per pull request, head commit and LLM language).
    AnalysisId
);
uuid_id!(
    /// Identifier of a job (one agent run).
    JobId
);
uuid_id!(
    /// Identifier of an Ask question.
    QuestionId
);
uuid_id!(
    /// Identifier of a feedback entry.
    FeedbackId
);

/// Identifier of a flow, assigned by the `plan` job: `f1`, `f2`, ….
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
pub struct FlowId(#[schemars(pattern(r"^f[0-9]+$"))] pub String);

impl fmt::Display for FlowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

string_id!(
    /// Identifier of a step, assigned by the server in step order: `"<flow>.<n>"`, e.g. `f2.3`.
    StepId
);
string_id!(
    /// Identifier of an annotation, assigned by the server: `"<step>.a<n>"`, e.g. `f2.3.a1`.
    AnnotationId
);
string_id!(
    /// Identifier of a diff hunk from the server's pre-pass: `h1`, `h2`, ….
    HunkId
);
string_id!(
    /// A full 40-character hexadecimal git commit id.
    CommitSha
);
