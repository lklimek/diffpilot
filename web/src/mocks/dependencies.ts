import type { DependencyView } from "../api/generated/types";

/** The Dependencies view of fixture PR 482 (one crate, registry dependencies only; ADR-021). */
export function mockDependencies(): DependencyView {
  return {
    status: "available",
    controlled_owners: ["acme"],
    limits: [],
    files_outside_crates: 0,
    notes: "produced",
    repositories: [
      {
        kind: "this",
        repo: { owner: "acme", name: "node-client" },
        location: null,
        controlled: true,
        crates: [
          {
            name: "node-client",
            version: "0.7.0",
            manifest: "Cargo.toml",
            source_at: null,
            revision: null,
            used_by: [],
            facts_about: 0,
            changes: {
              changed_lines: 212,
              files: 9,
              flows: [
                { id: "f1", title: "Send an RPC request with retry" },
                { id: "f2", title: "Default timeout 30 s → 5 s" },
              ],
              facts: [],
              facts_hidden: 0,
              other_facts: 0,
              notes: [
                {
                  text: "retry with backoff around `RpcClient::send`",
                  lines: [
                    { file: "src/rpc/client.rs", side: "head", range: { start_line: 40, end_line: 88 } },
                  ],
                  changed_lines: 120,
                },
              ],
              notes_hidden: 3,
              other_dependencies: 5,
            },
          },
        ],
      },
    ],
  };
}
