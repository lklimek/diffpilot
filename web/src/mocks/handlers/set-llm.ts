import { HttpResponse } from "msw";
import type { LlmJobSlots, LlmSelection, ModelChoice, SetAskEffortRequest } from "../../api/generated/types";
import { SETTINGS, scenario } from "../scenarios";
import { apiError, route } from "./common";
export let savedSelection: LlmSelection | undefined;
export let savedJobSlots: number | undefined;
/** Clears the signed-in user's choices between mock tests. */
export function resetSelection() {
  savedSelection = undefined;
  savedJobSlots = undefined;
}
/** The signed-in user's model selection. */
export const setLlm = route("setLlm", async ({ request }) => {
  if (scenario().llmSaveFails) return apiError(503, "unavailable", "Try again later.");
  const choice = (await request.json()) as ModelChoice;
  const selection = savedSelection ?? SETTINGS.llm_selection!;
  if (
    !selection.models.some((m) => m.id === choice.model) ||
    !selection.efforts.includes(choice.reasoning_effort)
  )
    return apiError(400, "invalid_request", "Choose an available model and effort.");
  savedSelection = {
    ...selection,
    current: choice,
    changed_by: "alice",
    changed_at: new Date().toISOString(),
  };
  return HttpResponse.json(savedSelection);
});
/** `PUT /api/v1/settings/llm/ask-effort`: kept with the signed-in user's selection. */
const saveEffort = (name: "setAskEffort" | "setJudgeEffort", field: "ask_effort" | "judge_effort") =>
  route(name, async ({ request }) => {
    if (scenario().llmSaveFails) return apiError(503, "unavailable", "Try again later.");
    const { reasoning_effort } = (await request.json()) as SetAskEffortRequest;
    const selection = savedSelection ?? SETTINGS.llm_selection!;
    if (!selection.efforts.includes(reasoning_effort))
      return apiError(400, "invalid_request", "Choose an available effort.");
    savedSelection = {
      ...selection,
      [field]: reasoning_effort,
      changed_by: "alice",
      changed_at: new Date().toISOString(),
    };
    return HttpResponse.json(savedSelection);
  });

export const setAskEffort = saveEffort("setAskEffort", "ask_effort");
export const setJudgeEffort = saveEffort("setJudgeEffort", "judge_effort");

export const setLlmJobSlots = route("setLlmJobSlots", async ({ request }) => {
  if (scenario().llmSaveFails) return apiError(503, "unavailable", "Try again later.");
  const { slots } = (await request.json()) as LlmJobSlots;
  if (!Number.isInteger(slots) || slots < 1 || slots > 65_535)
    return apiError(400, "invalid_request", "Enter a whole number from 1 to 65,535.");
  savedJobSlots = slots;
  return HttpResponse.json({ slots });
});
