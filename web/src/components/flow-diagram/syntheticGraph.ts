import type { FlowGraph, GraphEdge, GraphNode, NodeKind } from "../../api/generated/types";

const KINDS: NodeKind[] = ["unchanged", "changed", "added"];
const LABELS = [
  "Enumerate existing local identities",
  "Decide whether the modern record exists for this identity?",
  "Load the legacy record and keep its allowlist",
  "Count the failure and continue",
  "Return the recovered allowlist to the caller",
];

/** A synthetic flow of `count` steps for tests and screenshots: a main path with branches, loops and exits. */
export function syntheticGraph(count: number): FlowGraph {
  const id = (n: number) => `s1.${n}`;
  const nodes: GraphNode[] = Array.from({ length: count }, (_, i) => ({
    step_id: id(i + 1),
    number: i + 1,
    kind: KINDS[i % KINDS.length]!,
    label: `${LABELS[i % LABELS.length]} (${i + 1})`,
  }));
  const edges: GraphEdge[] = [];
  for (let n = 1; n < count; n += 1) {
    const branching = n % 4 === 2;
    edges.push({ from: id(n), to: id(n + 1), label: branching ? "record found and decoded" : null });
    if (branching && n + 2 <= count) {
      edges.push({ from: id(n), to: id(n + 2), label: "read error: count the failure and continue" });
    }
    if (n % 9 === 0) edges.push({ from: id(n), to: id(n - 6), label: "next identity (loop)" });
  }
  return { nodes, edges };
}
