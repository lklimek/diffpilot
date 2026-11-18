/** The button of a flow that is analysed again from scratch (its steps are replaced). */
export const ANALYSE_AGAIN = "Analyse this flow again";

/**
 * The sentence the server ends a failed flow's message with when the flow can be analysed again
 * (`crates/server/src/pipeline/tour.rs`, `crates/db/src/repo/flow_work.rs`; pinned by `again.test.ts`).
 */
export const ANALYSE_AGAIN_SENTENCE = `${ANALYSE_AGAIN}.`;
