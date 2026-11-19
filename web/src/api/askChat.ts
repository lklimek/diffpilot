// Ask chat shapes of the planned contract (AskAnchor `flow` and `pr`, `parent_id`), ahead of the generated types.
// TODO: replace with the generated `AskAnchor`, `AskQuestion` and `AnchoredAskRequest` once the contract branch
// (feat/ask-threads) is merged; these aliases then become re-exports.
import type { AnchoredAskRequest, AskAnchor, AskQuestion, FlowId, QuestionId } from "./generated/types";

/** What a chat question is about: a step (or lines of it), lines or a whole file, a flow, or the whole PR. */
export type ChatAnchor = AskAnchor | { kind: "flow"; flow_id: FlowId } | { kind: "pr" };

/** A question of the chat; a follow-up names the first question of its thread. */
export type ChatQuestion = Omit<AskQuestion, "anchor"> & { anchor: ChatAnchor; parent_id?: QuestionId };

/** Body of `POST /api/v1/analyses/{id}/questions` with a thread. */
export type ChatAskRequest = Omit<AnchoredAskRequest, "anchor"> & {
  anchor: ChatAnchor;
  parent_id?: QuestionId;
};
