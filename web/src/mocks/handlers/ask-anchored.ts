import type { AnchoredAskRequest } from "../../api/generated/types";
import { HttpResponse } from "msw";
import { seedFor } from "../fixtures";
import { readStore } from "../review-store";
import { apiError, forAnalysis, route } from "./common";
import { answerLater } from "./ask-question";

/**
 * `POST /api/v1/analyses/{id}/questions` (R14): 202 pending; the answer follows on the event stream. Like the
 * server, a follow-up joins its thread's first question and repeats its anchor; an unknown parent or flow is 404.
 */
export const askAnchored = route(
  "askAnchored",
  forAnalysis(async ({ request }) => {
    const { text, anchor, parent_id } = (await request.json()) as AnchoredAskRequest;
    const tour = seedFor("default", "tour") as { flows: { id: string }[] };
    if (anchor.kind === "flow" && !tour.flows.some((f) => f.id === anchor.flow_id))
      return apiError(404, "not_found", "This flow is not part of the analysis.");
    let root = undefined;
    if (parent_id) {
      const questions = readStore("questions");
      const parent = questions.find((q) => q.id === parent_id);
      root = parent?.parent_id ? questions.find((q) => q.id === parent.parent_id) : parent;
      if (!root) return apiError(404, "not_found", "This question is not in your conversation.");
      if (JSON.stringify(root.anchor) !== JSON.stringify(anchor))
        return apiError(400, "invalid_request", "A follow-up must repeat its thread's anchor.");
    }
    return HttpResponse.json(answerLater(anchor, text, root?.id), { status: 202 });
  }),
);
