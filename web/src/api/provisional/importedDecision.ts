// TODO(v1.4 phase B): replace with generated contract type (plan C11)
import type { DecisionKind } from "../generated/types";

export interface ImportedGithubDecision {
  via: "imported_github";
  /** Null when only the thread resolution is known. */
  kind: DecisionKind | null;
  by: string | null;
  at: string | null;
  evidence_url: string | null;
}
