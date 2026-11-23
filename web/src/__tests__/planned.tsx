import { useState, type ReactNode } from "react";
import type { DecisionBatchRow } from "../api/generated/types";
import type { EffectsPreviewState } from "../components/comments/decisions";
import { isRefused, previewEffects } from "../mocks/review-loop";

/** `rows` planned at once by the mocks' server model, as the screen's RC16 preview gives it. */
export function planOf(rows: DecisionBatchRow[] | null): EffectsPreviewState {
  if (rows === null) return { state: "idle" };
  const answer = previewEffects(rows);
  return isRefused(answer)
    ? { state: "error", message: answer[2] }
    : { state: "ready", items: answer, rows: JSON.stringify(rows), updating: false };
}

/** A card's draft planning as the Questions screen wires it: `preview` of the draft the card reports. */
export interface Plan {
  preview: EffectsPreviewState;
  onDraft: (rows: DecisionBatchRow[] | null) => void;
}

/** Renders a card whose drafts the mocks' server model plans at once (the screen asks RC16). */
export function Planned({ children }: { children: (plan: Plan) => ReactNode }) {
  const [rows, setRows] = useState<DecisionBatchRow[] | null>(null);
  return <>{children({ preview: planOf(rows), onDraft: setRows })}</>;
}
