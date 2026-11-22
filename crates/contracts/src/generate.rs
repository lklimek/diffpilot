//! Generated, checked-in artefacts (ADR-009): agent-output schemas, the TypeScript API types and routes, and the
//! JSON Schemas of what a personal access token reads (plan v1.1 §E.5 E5-7).
//! Built in memory; the drift test `generated_artifacts_are_current` compares or rewrites the files.

use std::any::TypeId;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use schemars::JsonSchema;
use ts_rs::{Config, TS, TypeVisitor};

use crate::agent::{JobKind, StrictSchemaError, output_schema};
use crate::api::access_tokens::{AccessToken, CreateAccessToken, CreatedAccessToken};
use crate::api::analyses::{AnalysisSummary, CreateAnalysisRequest, MyAnalyses, MyAnalysesQuery};
use crate::api::auth::{LogoutRequest, MeResponse};
use crate::api::code::{DiffResponse, LinesResponse, RangeQuery};
use crate::api::comments::{
    CreateCommentRequest, CreateCommentResponse, PendingReview, UpdateCommentRequest,
};
use crate::api::dashboard::{
    DashboardEvent, DashboardSettings, DashboardSettingsView, DashboardSummary, ItemPage,
    ItemQuery, RefreshStatus, SetTimeZoneRequest, SnoozeRequest, SummaryQuery,
};
use crate::api::deps::DependencyView;
use crate::api::error::{ApiError, LoginError};
use crate::api::events::AnalysisEvent;
use crate::api::feedback::{Feedback, FeedbackRequest};
use crate::api::files::{FilesResponse, SetViewedRequest};
use crate::api::pr::{ResolvePrRequest, ResolvePrResponse};
use crate::api::questions::{AnchoredAskRequest, AskQuestion, AskRequest};
use crate::api::review::ReviewState;
use crate::api::review_loop::{
    AgentPushPayload, CarryOverPage, CarryOverQuery, ChannelTestResult, Claim, ClaimRequest,
    CodeContext, CommentItem, CommentLog, CommentsOverview, ContextQuery, DecideRequest, Decision,
    DecisionBatchRequest, DecisionFeed, EffectReportRequest, FeedDecision, FeedEvent, FeedQuery,
    ItemsQuery, NotificationSettings, PrPage, PushTarget, PutChannelRequest, PutPushTargetRequest,
    UserEvent, WatchList, WebhookPayload,
};
use crate::api::settings::{
    RepoSettings, SetAskEffortRequest, SetControlledOwnersRequest, SetJudgeEffortRequest,
    SetLanguageRequest, Settings,
};
use crate::api::tour::Tour;
use crate::routes::{self, Listener, ROUTES, Route};

/// Directory of the agent-output schemas, relative to the repository root.
pub const SCHEMA_DIR: &str = "schemas/agent";
/// Directory of the generated TypeScript, relative to the repository root.
pub const TS_DIR: &str = "web/src/api/generated";
/// Directory of the JSON Schemas of what a token reads, relative to the repository root.
pub const API_SCHEMA_DIR: &str = "schemas/api";

/// One JSON Schema of [`API_SCHEMA_DIR`]: a response body (SSE: event data), a query or the error body.
#[derive(Clone, Copy, Debug)]
pub struct ApiSchema {
    /// File stem: `schemas/api/<stem>.schema.json`.
    pub stem: &'static str,
    /// Routes whose response body the schema describes; empty for a query and the error body.
    pub responses: &'static [Route],
    schema: fn() -> schemars::Schema,
}

fn schema_of<T: JsonSchema>() -> schemars::Schema {
    schemars::schema_for!(T)
}

const fn api_schema<T: JsonSchema>(stem: &'static str, responses: &'static [Route]) -> ApiSchema {
    ApiSchema {
        stem,
        responses,
        schema: schema_of::<T>,
    }
}

/// Every schema of [`API_SCHEMA_DIR`]: each route a token may read (`read` and `GET /api/v1/me`) names its
/// response here (contract test).
pub const API_SCHEMAS: &[ApiSchema] = &[
    api_schema::<ApiError>("api_error", &[]),
    api_schema::<MeResponse>("me_response", &[routes::GET_ME]),
    api_schema::<DashboardSummary>("dashboard_summary", &[routes::GET_DASHBOARD]),
    api_schema::<ItemQuery>("item_query", &[]),
    api_schema::<SummaryQuery>("summary_query", &[]),
    api_schema::<ItemPage>("item_page", &[routes::LIST_DASHBOARD_ITEMS]),
    api_schema::<RefreshStatus>("refresh_status", &[routes::GET_DASHBOARD_REFRESH]),
    api_schema::<DashboardEvent>("dashboard_event", &[routes::DASHBOARD_REFRESH_EVENTS]),
    api_schema::<DashboardSettingsView>(
        "dashboard_settings_view",
        &[routes::GET_DASHBOARD_SETTINGS],
    ),
    api_schema::<AnalysisSummary>("analysis_summary", &[routes::GET_ANALYSIS]),
    api_schema::<MyAnalysesQuery>("my_analyses_query", &[]),
    api_schema::<MyAnalyses>("my_analyses", &[routes::LIST_MY_ANALYSES]),
    api_schema::<AnalysisEvent>("analysis_event", &[routes::ANALYSIS_EVENTS]),
    api_schema::<Tour>("tour", &[routes::GET_TOUR]),
    api_schema::<LinesResponse>("lines_response", &[routes::GET_LINES]),
    api_schema::<DiffResponse>("diff_response", &[routes::GET_DIFF]),
    api_schema::<ReviewState>("review_state", &[routes::GET_REVIEW_STATE]),
    api_schema::<Vec<AskQuestion>>(
        "ask_question_list",
        &[routes::LIST_QUESTIONS, routes::LIST_PR_QUESTIONS],
    ),
    api_schema::<FilesResponse>("files_response", &[routes::GET_FILES]),
    api_schema::<DependencyView>("dependency_view", &[routes::GET_DEPENDENCIES]),
    api_schema::<Vec<Feedback>>("feedback_list", &[routes::LIST_FEEDBACK]),
    api_schema::<PendingReview>("pending_review", &[routes::GET_PENDING_REVIEW]),
    api_schema::<Settings>("settings", &[routes::GET_SETTINGS]),
    api_schema::<RepoSettings>("repo_settings", &[routes::GET_REPO_SETTINGS]),
    api_schema::<PrPage>("pr_page", &[routes::GET_PR_PAGE]),
    api_schema::<CommentsOverview>("comments_overview", &[routes::GET_COMMENTS]),
    api_schema::<ItemsQuery>("items_query", &[]),
    api_schema::<Vec<CommentItem>>("comment_item_list", &[routes::LIST_COMMENT_ITEMS]),
    api_schema::<ContextQuery>("context_query", &[]),
    api_schema::<CodeContext>("code_context", &[routes::GET_COMMENT_CONTEXT]),
    api_schema::<CommentLog>("comment_log", &[routes::GET_COMMENT_LOG]),
    api_schema::<CarryOverQuery>("carry_over_query", &[]),
    api_schema::<CarryOverPage>("carry_over_page", &[routes::LIST_CARRY_OVERS]),
    api_schema::<WatchList>("watch_list", &[routes::LIST_WATCHES]),
    api_schema::<UserEvent>("user_event", &[routes::USER_EVENTS]),
    api_schema::<FeedQuery>("feed_query", &[]),
    api_schema::<DecisionFeed>("decision_feed", &[routes::GET_DECISION_FEED]),
    api_schema::<FeedDecision>("feed_decision", &[routes::GET_FEED_DECISION]),
    api_schema::<FeedEvent>("feed_event", &[routes::DECISION_FEED_EVENTS]),
];

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
    #[error("a JSON Schema could not be rendered as JSON")]
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
    for api in API_SCHEMAS {
        files.push(GeneratedFile {
            path: format!("{API_SCHEMA_DIR}/{}.schema.json", api.stem),
            contents: serde_json::to_string_pretty(&(api.schema)())? + "\n",
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
    collector.visit::<MyAnalysesQuery>();
    collector.visit::<MyAnalyses>();
    collector.visit::<crate::api::analyses::BackgroundAnalysisRequest>();
    collector.visit::<AnalysisEvent>();
    collector.visit::<Tour>();
    collector.visit::<RangeQuery>();
    collector.visit::<LinesResponse>();
    collector.visit::<DiffResponse>();
    collector.visit::<ReviewState>();
    collector.visit::<AskRequest>();
    collector.visit::<AskQuestion>();
    collector.visit::<AnchoredAskRequest>();
    collector.visit::<FilesResponse>();
    collector.visit::<SetViewedRequest>();
    collector.visit::<DependencyView>();
    collector.visit::<FeedbackRequest>();
    collector.visit::<Feedback>();
    collector.visit::<PendingReview>();
    collector.visit::<CreateCommentRequest>();
    collector.visit::<CreateCommentResponse>();
    collector.visit::<UpdateCommentRequest>();
    collector.visit::<Settings>();
    collector.visit::<SetLanguageRequest>();
    collector.visit::<SetControlledOwnersRequest>();
    collector.visit::<RepoSettings>();
    collector.visit::<SetJudgeEffortRequest>();
    collector.visit::<SetAskEffortRequest>();
    collector.visit::<DashboardSummary>();
    collector.visit::<ItemQuery>();
    collector.visit::<SummaryQuery>();
    collector.visit::<ItemPage>();
    collector.visit::<RefreshStatus>();
    collector.visit::<DashboardEvent>();
    collector.visit::<DashboardSettings>();
    collector.visit::<DashboardSettingsView>();
    collector.visit::<SetTimeZoneRequest>();
    collector.visit::<SnoozeRequest>();
    collector.visit::<AccessToken>();
    collector.visit::<CreateAccessToken>();
    collector.visit::<CreatedAccessToken>();
    collector.visit::<PrPage>();
    collector.visit::<CommentsOverview>();
    collector.visit::<ItemsQuery>();
    collector.visit::<CommentItem>();
    collector.visit::<ContextQuery>();
    collector.visit::<CodeContext>();
    collector.visit::<DecideRequest>();
    collector.visit::<DecisionBatchRequest>();
    collector.visit::<Decision>();
    collector.visit::<CommentLog>();
    collector.visit::<CarryOverQuery>();
    collector.visit::<CarryOverPage>();
    collector.visit::<WatchList>();
    collector.visit::<UserEvent>();
    collector.visit::<NotificationSettings>();
    collector.visit::<PutChannelRequest>();
    collector.visit::<ChannelTestResult>();
    collector.visit::<PushTarget>();
    collector.visit::<PutPushTargetRequest>();
    collector.visit::<FeedQuery>();
    collector.visit::<DecisionFeed>();
    collector.visit::<FeedDecision>();
    collector.visit::<FeedEvent>();
    collector.visit::<ClaimRequest>();
    collector.visit::<Claim>();
    collector.visit::<EffectReportRequest>();
    collector.visit::<WebhookPayload>();
    collector.visit::<AgentPushPayload>();
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
