//! GraphQL alias batches: lookups `a0`, `a1`, … in one query with `rateLimit`; a batch failing with a 5xx, a
//! non-JSON answer or a timeout is retried once as two halves (S11-2: oversized batches answer an HTML page or
//! 504).

use futures::future::try_join_all;
use serde_json::Value;

use super::client::{TokenSource, WorkClient, WorkError};

/// Pull requests per detail batch (S11-2: ≈1.1 points each, 5–8 s and ~0.6 MB per batch of 10).
pub const PR_BATCH: usize = 10;
/// GitHub fails batches of 30 and more pull requests; never send more than this many.
pub const MAX_PR_BATCH: usize = 20;
const _: () = assert!(PR_BATCH <= MAX_PR_BATCH);

/// A JSON string literal, valid GraphQL string syntax (JSON escapes are a subset of GraphQL's).
pub(crate) fn literal(text: &str) -> String {
    Value::from(text).to_string()
}

/// The query of `lookups`, each a top-level field without alias (`repository(…) { … }`), aliased `a<i>`.
pub(crate) fn query(fragments: &str, lookups: &[String]) -> String {
    let mut document = String::from("query {");
    for (index, lookup) in lookups.iter().enumerate() {
        document.push_str(&format!(" a{index}: {lookup}"));
    }
    document.push_str(" rateLimit { cost remaining resetAt } }");
    document.push_str(fragments);
    document
}

/// What a batched lookup answered: each key with its lookup's value (`null` when GitHub did not resolve it), and
/// the keys whose batch and half both failed.
pub(crate) struct Answers<K> {
    pub(crate) resolved: Vec<(K, Value)>,
    pub(crate) failed: Vec<K>,
}

impl<T: TokenSource> WorkClient<T> {
    /// Runs one lookup per key in batches of `size`, batches concurrently (the client limits calls in flight).
    /// Errors that end the refresh are returned; others leave the batch's keys in `failed`.
    pub(crate) async fn batched<K: Clone>(
        &self,
        keys: &[K],
        size: usize,
        fragments: &str,
        lookup: impl Fn(&K) -> String,
    ) -> Result<Answers<K>, WorkError> {
        let lookup = &lookup;
        let parts = try_join_all(
            keys.chunks(size.max(1))
                .map(|chunk| self.batch(chunk, fragments, lookup)),
        )
        .await?;
        let mut answers = Answers {
            resolved: Vec::new(),
            failed: Vec::new(),
        };
        for part in parts {
            answers.resolved.extend(part.resolved);
            answers.failed.extend(part.failed);
        }
        Ok(answers)
    }

    async fn batch<K: Clone>(
        &self,
        chunk: &[K],
        fragments: &str,
        lookup: &impl Fn(&K) -> String,
    ) -> Result<Answers<K>, WorkError> {
        match self.once(chunk, fragments, lookup).await {
            Ok(resolved) => Ok(Answers {
                resolved,
                failed: Vec::new(),
            }),
            Err(error) if error.ends_refresh() => Err(error),
            Err(error) if error.splits_batch() && chunk.len() > 1 => {
                tracing::debug!(
                    size = chunk.len(),
                    ?error,
                    "a GitHub GraphQL batch failed; retrying it once as two halves"
                );
                let (first, second) = chunk.split_at(chunk.len() / 2);
                let mut answers = Answers {
                    resolved: Vec::new(),
                    failed: Vec::new(),
                };
                for half in [first, second] {
                    match self.once(half, fragments, lookup).await {
                        Ok(resolved) => answers.resolved.extend(resolved),
                        Err(error) if error.ends_refresh() => return Err(error),
                        Err(error) => {
                            tracing::warn!(
                                size = half.len(),
                                ?error,
                                "half of a GitHub GraphQL batch failed again; its lookups are left out"
                            );
                            answers.failed.extend_from_slice(half);
                        }
                    }
                }
                Ok(answers)
            }
            Err(error) => {
                tracing::warn!(
                    size = chunk.len(),
                    ?error,
                    "a GitHub GraphQL batch failed; its lookups are left out"
                );
                Ok(Answers {
                    resolved: Vec::new(),
                    failed: chunk.to_vec(),
                })
            }
        }
    }

    async fn once<K: Clone>(
        &self,
        chunk: &[K],
        fragments: &str,
        lookup: &impl Fn(&K) -> String,
    ) -> Result<Vec<(K, Value)>, WorkError> {
        let lookups: Vec<String> = chunk.iter().map(lookup).collect();
        let mut data = self.graphql(&query(fragments, &lookups)).await?;
        Ok(chunk
            .iter()
            .enumerate()
            .map(|(index, key)| {
                let value = data.remove(&format!("a{index}")).unwrap_or(Value::Null);
                (key.clone(), value)
            })
            .collect())
    }
}
