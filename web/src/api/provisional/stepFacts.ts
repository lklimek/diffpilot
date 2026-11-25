import type { LineRange, StepFacts } from "../generated/types";

// TODO(v1.4 phase B): replace with generated contract type (plan C16)
export type StepFactsWithLeadingContext = StepFacts & { leading_context?: LineRange | null };
