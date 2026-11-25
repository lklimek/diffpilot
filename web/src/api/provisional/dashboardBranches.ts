// TODO(v1.4 phase B): replace with generated contract type (plan C5)
export interface WorkItemBranches {
  stack: { position: "bottom" | "middle" | "top"; size: number; dependents: number[] } | null;
  release_base: string | null;
}
