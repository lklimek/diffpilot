//! Generated, checked-in artefacts (ADR-009): agent-output schemas and the TypeScript API types and routes.
//! Built in memory; the drift test `generated_artifacts_are_current` compares or rewrites the files.

use std::any::TypeId;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use ts_rs::{Config, TS, TypeVisitor};

use crate::agent::{JobKind, StrictSchemaError, output_schema};
use crate::api::analyses::{AnalysisSummary, CreateAnalysisRequest};
use crate::api::auth::{LogoutRequest, MeResponse};
use crate::api::code::{DiffResponse, LinesResponse, RangeQuery};
use crate::api::comments::{
    CreateCommentRequest, CreateCommentResponse, PendingReview, UpdateCommentRequest,
};
use crate::api::error::{ApiError, LoginError};
use crate::api::events::AnalysisEvent;
use crate::api::feedback::{Feedback, FeedbackRequest};
use crate::api::pr::{ResolvePrRequest, ResolvePrResponse};
use crate::api::questions::{AskQuestion, AskRequest};
use crate::api::review::ReviewState;
use crate::api::settings::{RepoSettings, SetLanguageRequest, Settings};
use crate::api::tour::Tour;
use crate::routes::{Listener, ROUTES};

/// Directory of the agent-output schemas, relative to the repository root.
pub const SCHEMA_DIR: &str = "schemas/agent";
/// Directory of the generated TypeScript, relative to the repository root.
pub const TS_DIR: &str = "web/src/api/generated";

const NOTE: &str = "This file is generated from crates/contracts by the test generated_artifacts_are_current \
                    (UPDATE_GENERATED=1 rewrites it). Do not edit.";

/// One generated file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    /// Path relative to the repository root.
    pub path: String,
    /// Full contents.
    pub contents: String,
}

/// Generation failed.
#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    /// An agent schema is outside the Codex strict subset.
    #[error("agent schema for {kind} is outside the Codex strict subset")]
    Schema {
        /// The job kind.
        kind: &'static str,
        /// The violation.
        #[source]
        source: StrictSchemaError,
    },
    /// A schema could not be rendered as JSON.
    #[error("agent schema could not be rendered as JSON")]
    Json(#[from] serde_json::Error),
    /// Two Rust types map to one TypeScript name.
    #[error("two contract types are both named `{0}` in TypeScript")]
    DuplicateTypeName(String),
}

/// Every generated file, in a fixed order.
pub fn generated_files() -> Result<Vec<GeneratedFile>, GenerateError> {
    let mut files = Vec::new();
    for kind in JobKind::ALL {
        let schema = output_schema(kind).map_err(|source| GenerateError::Schema {
            kind: kind.as_str(),
            source,
        })?;
        files.push(GeneratedFile {
            path: format!("{SCHEMA_DIR}/{}.schema.json", kind.as_str()),
            contents: serde_json::to_string_pretty(&schema)? + "\n",
        });
    }
    files.push(GeneratedFile {
        path: format!("{TS_DIR}/types.ts"),
        contents: typescript_types()?,
    });
    files.push(GeneratedFile {
        path: format!("{TS_DIR}/routes.ts"),
        contents: typescript_routes(),
    });
    Ok(files)
}

/// Collects the declaration of every exportable type reachable from the roots.
struct Collector {
    config: Config,
    seen: HashSet<TypeId>,
    declarations: BTreeMap<String, String>,
    duplicate: Option<String>,
}

impl TypeVisitor for Collector {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if T::output_path().is_none() || !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        let name = T::ident(&self.config);
        let declaration = format!(
            "{}export {}",
            T::docs().unwrap_or_default(),
            T::decl(&self.config)
        );
        if self
            .declarations
            .insert(name.clone(), declaration)
            .is_some()
        {
            self.duplicate.get_or_insert(name);
        }
        T::visit_dependencies(self);
    }
}

fn typescript_types() -> Result<String, GenerateError> {
    let mut collector = Collector {
        config: Config::new(),
        seen: HashSet::new(),
        declarations: BTreeMap::new(),
        duplicate: None,
    };
    collector.visit::<MeResponse>();
    collector.visit::<LogoutRequest>();
    collector.visit::<LoginError>();
    collector.visit::<ApiError>();
    collector.visit::<ResolvePrRequest>();
    collector.visit::<ResolvePrResponse>();
    collector.visit::<CreateAnalysisRequest>();
    collector.visit::<AnalysisSummary>();
    collector.visit::<crate::api::analyses::BackgroundAnalysisRequest>();
    collector.visit::<AnalysisEvent>();
    collector.visit::<Tour>();
    collector.visit::<RangeQuery>();
    collector.visit::<LinesResponse>();
    collector.visit::<DiffResponse>();
    collector.visit::<ReviewState>();
    collector.visit::<AskRequest>();
    collector.visit::<AskQuestion>();
    collector.visit::<FeedbackRequest>();
    collector.visit::<Feedback>();
    collector.visit::<PendingReview>();
    collector.visit::<CreateCommentRequest>();
    collector.visit::<CreateCommentResponse>();
    collector.visit::<UpdateCommentRequest>();
    collector.visit::<Settings>();
    collector.visit::<SetLanguageRequest>();
    collector.visit::<RepoSettings>();
    if let Some(name) = collector.duplicate {
        return Err(GenerateError::DuplicateTypeName(name));
    }
    let mut out = format!("// {NOTE}\n");
    for declaration in collector.declarations.values() {
        out.push('\n');
        for line in declaration.lines() {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    Ok(out)
}

fn typescript_routes() -> String {
    let mut out = format!(
        "// {NOTE}\n\n/** Browser-facing routes: method, path with `{{param}}` placeholders, authentication. */\n\
         export const routes = {{\n"
    );
    for route in ROUTES
        .iter()
        .filter(|route| route.listener == Listener::Public)
    {
        // Writing to a String cannot fail.
        let _ = writeln!(
            out,
            "  {}: {{ method: \"{}\", path: \"{}\", auth: \"{}\" }},",
            route.name,
            route.method.as_str(),
            route.path,
            route.auth.as_str()
        );
    }
    out.push_str("} as const;\n\n/** Name of a browser-facing route. */\nexport type RouteName = keyof typeof routes;\n");
    out
}
