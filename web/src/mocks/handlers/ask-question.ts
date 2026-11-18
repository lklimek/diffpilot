import { HttpResponse } from "msw";
import type { AskQuestion, AskRequest } from "../../api/generated/types";
import { ANSWER_MS, publishQuestion, updateStore } from "../review-store";
import { forAnalysis, route } from "./common";

const ANSWER =
  "Mock answer: I read the lines of this step and their callers. Does any caller rely on the earlier behaviour?";

/** `POST /api/v1/analyses/{id}/steps/{step_id}/questions`: 202 pending; the answer follows on the event stream. */
export const askQuestion = route(
  "askQuestion",
  forAnalysis(async ({ request, params }) => {
    const { text } = (await request.json()) as AskRequest;
    const question: AskQuestion = {
      id: crypto.randomUUID(),
      step_id: String(params.step_id),
      text,
      state: "pending",
      mine: true,
      asked_by: "alice",
      answer: null,
      failure: null,
      created_at: new Date().toISOString(),
    };
    const save = (q: AskQuestion) =>
      updateStore((store) => {
        store.questions = [...store.questions.filter((x) => x.id !== q.id), q];
      });
    save(question);
    setTimeout(() => {
      const answered: AskQuestion = {
        ...question,
        state: "answered",
        answer: { answer: ANSWER, evidence: [] },
      };
      save(answered);
      publishQuestion(answered);
    }, ANSWER_MS);
    return HttpResponse.json(question, { status: 202 });
  }),
);
