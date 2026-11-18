import type { Annotation, AnnotationId, RiskBand, Step } from "../../api/generated/types";

/** Remarks of one band: how many, and how many of them have the user's feedback. */
export interface RemarkCount {
  band: RiskBand;
  count: number;
  handled: number;
}

const ORDER: RiskBand[] = ["high", "medium", "low"];

// A remark's severity on the wire is its step's LLM risk band (as the code view's label draws it); remarks of a
// step the LLM did not rate have none and are not counted.
const bandOf = (step: Step): RiskBand | null => step.llm.assessment?.risk ?? null;

/** Remarks of `steps` by band, highest first; bands without remarks are left out (never a zero). */
export function remarkCounts(steps: readonly Step[], handled: ReadonlySet<AnnotationId>): RemarkCount[] {
  return ORDER.flatMap((band) => {
    const remarks = steps.filter((s) => bandOf(s) === band).flatMap((s) => s.llm.annotations);
    return remarks.length === 0
      ? []
      : [{ band, count: remarks.length, handled: remarks.filter((a) => handled.has(a.id)).length }];
  });
}

/** The first remark of `band` in step order (by line within a step), one without feedback first. */
export function firstRemark(
  steps: readonly Step[],
  band: RiskBand,
  handled: ReadonlySet<AnnotationId>,
): { step: Step; annotation: Annotation } | undefined {
  const all = steps
    .filter((s) => bandOf(s) === band)
    .flatMap((step) =>
      [...step.llm.annotations]
        .sort((a, b) => a.range.start_line - b.range.start_line)
        .map((annotation) => ({ step, annotation })),
    );
  return all.find((r) => !handled.has(r.annotation.id)) ?? all[0];
}
