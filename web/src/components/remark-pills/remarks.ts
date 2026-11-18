import type { Annotation, AnnotationId, RiskBand, Step } from "../../api/generated/types";

/**
 * The LLM's rating of steps and their remark count, two separate facts: on the wire a remark has no severity of
 * its own, only its step has a rating (`assessment.risk`). For one step: its rating; for several: the highest.
 */
export interface RemarkSummary {
  /** Highest LLM rating among the steps; null when none is rated. */
  rating: RiskBand | null;
  /** Steps at `rating`. */
  rated: number;
  /** LLM remarks on all the steps, rated or not. */
  remarks: number;
  /** Remarks with the user's feedback. */
  handled: number;
  /** Steps per rating, highest first, unrated (`null`) last; ratings without steps left out. */
  ratings: { rating: RiskBand | null; steps: number }[];
}

const ORDER: (RiskBand | null)[] = ["high", "medium", "low", null];

const ratingOf = (step: Step): RiskBand | null => step.llm.assessment?.risk ?? null;

/** Rating and remarks of `steps`; null with neither (never an all-clear). */
export function summariseRemarks(
  steps: readonly Step[],
  handled: ReadonlySet<AnnotationId>,
): RemarkSummary | null {
  const remarks = steps.flatMap((s) => s.llm.annotations);
  const ratings = ORDER.flatMap((rating) => {
    const count = steps.filter((s) => ratingOf(s) === rating).length;
    return count === 0 ? [] : [{ rating, steps: count }];
  });
  const top = ratings.find((r) => r.rating !== null);
  if (!top && remarks.length === 0) return null;
  return {
    rating: top?.rating ?? null,
    rated: top?.steps ?? 0,
    remarks: remarks.length,
    handled: remarks.filter((a) => handled.has(a.id)).length,
    ratings,
  };
}

/**
 * Where a pill leads: the first remark (one without feedback first, by line) on a step at the highest rating, else
 * on any step; without remarks the first step at the highest rating (annotation null).
 */
export function remarkTarget(
  steps: readonly Step[],
  handled: ReadonlySet<AnnotationId>,
): { step: Step; annotation: Annotation | null } | undefined {
  const top = summariseRemarks(steps, handled)?.rating ?? null;
  const remarksOf = (list: readonly Step[]) =>
    list.flatMap((step) =>
      [...step.llm.annotations]
        .sort((a, b) => a.range.start_line - b.range.start_line)
        .map((annotation) => ({ step, annotation })),
    );
  const atTop = top === null ? [] : steps.filter((s) => ratingOf(s) === top);
  const all = remarksOf(atTop).length > 0 ? remarksOf(atTop) : remarksOf(steps);
  const found = all.find((r) => !handled.has(r.annotation.id)) ?? all[0];
  if (found) return found;
  return atTop[0] && { step: atTop[0], annotation: null };
}
