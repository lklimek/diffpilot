import { HttpResponse } from "msw";
import type { AnchoredAskRequest } from "../../api/generated/types";
import { forAnalysis, route } from "./common";
import { answerLater } from "./ask-question";

/** `POST /api/v1/analyses/{id}/questions` (R14): 202 pending; the answer follows on the event stream. */
export const askAnchored = route(
  "askAnchored",
  forAnalysis(async ({ request }) => {
    const { text, anchor } = (await request.json()) as AnchoredAskRequest;
    const question = answerLater(anchor, text);
    return HttpResponse.json(question, { status: 202 });
  }),
);
