//! Context packs (brief §5 Agent sessions): the stored results of an analysis that seed a new agent session when
//! the analysis's session is missing or unusable. Built at claim ([`StoredPacks`]), so a job that waited sees every
//! result stored before it starts; shared with Ask. The text comes from `prompts::context_pack`.

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::plan::PlanOutput;
use contracts::agent::pr_context::PrContextOutput;
use contracts::api::pr::PrSummary;
use contracts::ids::AnalysisId;
use db::error::DbError;
use db::repo::Db;
use db::repo::documents::DocumentKind;

use futures::future::BoxFuture;

use crate::jobs::claim::{ContextPackError, ContextPacks};
use crate::prompts::{self, AskContext, ContextPackInput, PromptError};

/// The stored agent results of one analysis.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredResults {
    /// The plan (merged over its batches), once stored.
    pub plan: Option<PlanOutput>,
    /// The PR context, once stored.
    pub pr_context: Option<PrContextOutput>,
    /// The accepted flow analyses, in flow-id order.
    pub flows: Vec<FlowAnalysisOutput>,
}

impl StoredResults {
    /// Reads the stored results of `analysis`.
    pub async fn load(db: &Db, analysis: AnalysisId) -> Result<Self, DbError> {
        let documents = db.documents();
        Ok(Self {
            plan: documents.get(analysis, DocumentKind::Plan, "").await?,
            pr_context: documents.get(analysis, DocumentKind::PrContext, "").await?,
            flows: documents
                .list(analysis, DocumentKind::Flow)
                .await?
                .into_iter()
                .map(|(_, flow)| flow)
                .collect(),
        })
    }

    /// Nothing stored yet.
    pub fn is_empty(&self) -> bool {
        self.plan.is_none() && self.pr_context.is_none() && self.flows.is_empty()
    }
}

/// The context pack of `results` (plus an Ask job's step context): `None` when there is nothing to seed a new
/// session with (the analysis's first job).
pub fn pack(
    pr: &PrSummary,
    results: &StoredResults,
    ask: Option<AskContext<'_>>,
) -> Result<Option<String>, PromptError> {
    if results.is_empty() && ask.is_none() {
        return Ok(None);
    }
    prompts::context_pack(&ContextPackInput {
        pr,
        plan: results.plan.as_ref(),
        pr_context: results.pr_context.as_ref(),
        flows: &results.flows,
        ask,
    })
    .map(Some)
}

/// The claim path's context packs: the analysis's results stored at claim time.
#[derive(Clone, Debug)]
pub struct StoredPacks {
    /// The database.
    pub db: Db,
}

impl ContextPacks for StoredPacks {
    fn pack(
        &self,
        analysis: AnalysisId,
    ) -> BoxFuture<'_, Result<Option<String>, ContextPackError>> {
        Box::pin(async move {
            let summary = self.db.analyses().summary(analysis).await?;
            let results = StoredResults::load(&self.db, analysis).await?;
            Ok(pack(&summary.pr, &results, None)?)
        })
    }
}
