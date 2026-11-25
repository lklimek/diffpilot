import { http, HttpResponse } from "msw";
import { LLM_ACTIVITY_PATH } from "../../api/provisional/llmActivity";
import { idleActivity } from "../llm-activity";
import { scenario } from "../scenarios";

export const getLlmActivity = http.get(LLM_ACTIVITY_PATH, () =>
  HttpResponse.json(scenario().llmActivity ?? idleActivity),
);
