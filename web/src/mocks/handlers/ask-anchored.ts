import { HttpResponse } from "msw";
import type { ChatAskRequest } from "../../api/askChat";
import { forAnalysis, route } from "./common";
import { answerLater } from "./ask-question";

/** `POST /api/v1/analyses/{id}/questions` (R14): 202 pending; the answer follows on the event stream. */
export const askAnchored = route(
  "askAnchored",
  forAnalysis(async ({ request }) => {
    const { text, anchor, parent_id } = (await request.json()) as ChatAskRequest;
    const question = answerLater(anchor, text, parent_id);
    return HttpResponse.json(question, { status: 202 });
  }),
);
