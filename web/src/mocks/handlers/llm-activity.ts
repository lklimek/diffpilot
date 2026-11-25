import { HttpResponse } from "msw";
import { route } from "./common";
import { idleActivity } from "../llm-activity";
import { scenario } from "../scenarios";

export const getLlmActivity = route("getLlmActivity", () =>
  HttpResponse.json(scenario().llmActivity ?? idleActivity),
);
