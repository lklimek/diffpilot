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

const LOCK = (line: number) => ({ file: "Cargo.lock", line });

/**
 * A multi-repository map: this repository's two crates (one changed, with facts for types of a sibling crate and
 * of controlled repositories), acme/node-sdk (same owner), dashpay/platform (controlled in Settings),
 * a registry and an unread path dependency.
 */
export function mockDependenciesMulti(): DependencyView {
  const view = mockDependencies();
  const [node] = view.repositories[0]!.crates;
  node!.changes = {
    ...node!.changes!,
    facts: [
      {
        kind: "from_impl",
        at: { file: "src/rpc/error.rs", line: 12 },
        type_name: "Status",
        written: "rpc_types::Status",
        type_crate: "rpc-types",
        imported_at: { file: "src/rpc/error.rs", line: 3 },
        definition: { kind: "other_repository" },
      },
      {
        kind: "try_from_impl",
        at: { file: "src/rpc/client.rs", line: 91 },
        type_name: "TransitionType",
        written: "TransitionType",
        type_crate: "dpp",
        imported_at: { file: "src/rpc/client.rs", line: 9 },
        definition: { kind: "other_repository" },
      },
      {
        kind: "conversion_fn",
        at: { file: "src/rpc/retry.rs", line: 21 },
        type_name: "NodeEvent",
        written: "node_proto::NodeEvent",
        type_crate: "node-proto",
        imported_at: null,
        definition: { kind: "located", at: { file: "crates/node-proto/src/event.rs", line: 8 } },
      },
      {
        kind: "impl_on_foreign_type",
        at: { file: "src/rpc/metrics.rs", line: 41 },
        type_name: "Peer",
        written: "node_proto::Peer",
        type_crate: "node-proto",
        imported_at: { file: "src/rpc/metrics.rs", line: 2 },
        definition: { kind: "not_located" },
      },
    ],
    facts_hidden: 0,
    other_facts: 2,
  };
  view.controlled_owners = ["acme", "dashpay"];
  view.files_outside_crates = 2;
  view.repositories[0]!.crates.push({
    name: "node-proto",
    version: "0.7.0",
    manifest: "crates/node-proto/Cargo.toml",
    source_at: null,
    revision: null,
    used_by: ["node-client"],
    facts_about: 2,
    changes: null,
  });
  view.repositories.push(
    {
      kind: "github",
      repo: { owner: "acme", name: "node-sdk" },
      location: "https://github.com/acme/node-sdk",
      controlled: true,
      crates: [
        {
          name: "rpc-types",
          version: "0.3.1",
          manifest: null,
          source_at: LOCK(30),
          revision: "4f2a9c1e8b7d6a5f4e3d2c1b0a9f8e7d6c5b4a39",
          used_by: ["node-client"],
          facts_about: 1,
          changes: null,
        },
      ],
    },
    {
      kind: "github",
      repo: { owner: "dashpay", name: "platform" },
      location: "https://github.com/dashpay/platform",
      controlled: true,
      crates: [
        {
          name: "dpp",
          version: "2.1.0",
          manifest: null,
          source_at: LOCK(42),
          revision: "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
          used_by: ["node-client"],
          facts_about: 1,
          changes: null,
        },
      ],
    },
    {
      kind: "registry",
      repo: null,
      location: "crates.io",
      controlled: false,
      crates: [
        {
          name: "tokio",
          version: "1.48.0",
          manifest: null,
          source_at: LOCK(36),
          revision: null,
          used_by: ["node-client"],
          facts_about: 0,
          changes: null,
        },
      ],
    },
    {
      kind: "path",
      repo: null,
      location: "../vendored/hdrhist",
      controlled: false,
      crates: [
        {
          name: "hdrhist",
          version: null,
          manifest: null,
          source_at: { file: "Cargo.toml", line: 24 },
          revision: null,
          used_by: ["node-client"],
          facts_about: 0,
          changes: null,
        },
      ],
    },
  );
  return view;
}

/** No `Cargo.toml` at the head commit. */
export const mockDependenciesNotRust = (): DependencyView => ({
  status: "not_rust",
  controlled_owners: ["acme"],
  limits: [],
  files_outside_crates: 13,
  notes: "produced",
  repositories: [],
});

/** Every limit of the map. */
export function mockDependenciesLimits(): DependencyView {
  return {
    ...mockDependenciesMulti(),
    limits: [
      { kind: "manifests_cut", total: 512, read: 400 },
      { kind: "manifest_unreadable", path: "crates/broken/Cargo.toml" },
      { kind: "lock_unreadable", path: "tools/Cargo.lock" },
      { kind: "no_lock_file", workspace_dir: "tools" },
      { kind: "no_lock_file", workspace_dir: "" },
      { kind: "source_unread", path: "src/rpc/generated_codes.rs" },
      { kind: "facts_cut", kept: 200 },
      { kind: "workspace_unresolved", manifest: "tools/xtask/Cargo.toml" },
      { kind: "target_specific", manifest: "Cargo.toml" },
      { kind: "source_cut", path: "src/rpc/generated_codes.rs" },
      { kind: "definitions_unread", dir: "crates/node-sdk", files: 2 },
      { kind: "workspace_entries_cut", manifest: "Cargo.toml" },
    ],
  };
}

/** The map with the notes in state `notes`; `produced` means produced, but none for any crate. */
export function mockDependenciesNotes(notes: DependencyView["notes"]): DependencyView {
  const view = mockDependenciesMulti();
  view.notes = notes;
  for (const crate of view.repositories.flatMap((r) => r.crates))
    if (crate.changes) Object.assign(crate.changes, { notes: [], notes_hidden: 0 });
  return view;
}

/** Only the pull request's owner is controlled (the Settings list is empty). */
export function mockDependenciesNoControlled(): DependencyView {
  const view = mockDependenciesMulti();
  view.controlled_owners = ["acme"];
  const platform = view.repositories.find((r) => r.repo?.owner === "dashpay")!;
  platform.controlled = false;
  platform.crates[0]!.facts_about = 0;
  // Facts about types of crates neither in this repository nor controlled are counted, not listed.
  const changes = view.repositories[0]!.crates[0]!.changes!;
  changes.facts = changes.facts.filter((f) => f.type_crate !== "dpp");
  changes.other_facts += 1;
  return view;
}

/** Only files outside any crate changed (docs, README). */
export function mockDependenciesNoChangedCrate(): DependencyView {
  const view = mockDependencies();
  view.files_outside_crates = 3;
  view.notes = "produced";
  view.repositories[0]!.crates[0]!.changes = null;
  return view;
}
