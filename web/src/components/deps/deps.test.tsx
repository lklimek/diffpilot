import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import type { CrateChanges, DependencyView } from "../../api/generated/types";
import {
  mockDependencies,
  mockDependenciesLimits,
  mockDependenciesMulti,
  mockDependenciesNoChangedCrate,
  mockDependenciesNoControlled,
  mockDependenciesNotes,
  mockDependenciesNotRust,
} from "../../mocks/dependencies";
import { CrateFacts } from "./CrateFacts";
import { CrateNotes } from "./CrateNotes";
import { DependencyNotices } from "./DependencyNotices";
import { DependencyTree } from "./DependencyTree";
import { crateKey, repositoryKey, type DepsLinks, type LineTarget } from "./model";

// Files of the pull request open in the Files view; any other line opens on GitHub at the head commit.
const PR_FILES = new Set(["src/rpc/error.rs", "src/rpc/client.rs", "src/rpc/retry.rs", "src/rpc/metrics.rs"]);

function links(onNavigate: (href: string) => void = vi.fn()): DepsLinks {
  return {
    line: (t: LineTarget) =>
      PR_FILES.has(t.file)
        ? { href: `/files#${t.file}:${t.side}:${t.line}${t.end ? `-${t.end}` : ""}`, external: false }
        : { href: `https://github.com/acme/node-client/blob/HEAD/${t.file}#L${t.line}`, external: true },
    flow: (id) => `/flows#${id}`,
    settings: "/settings",
    onNavigate,
  };
}

const changesOf = (view: DependencyView) => view.repositories[0]!.crates[0]!.changes!;

function Tree({ view, onNavigate = vi.fn() }: { view: DependencyView; onNavigate?: (href: string) => void }) {
  const [open, setOpen] = useState<Record<string, boolean>>({});
  return (
    <DependencyTree
      view={view}
      open={open}
      onToggle={(key, value) => setOpen((o) => ({ ...o, [key]: value }))}
      links={links(onNavigate)}
      showUnchanged
      targetCrate={null}
    />
  );
}

describe("DependencyNotices (brief 2026-11-20: Not available, limits, controlled owners)", () => {
  it("not_rust: says the view is not available and nothing was checked, never that there are no dependencies", () => {
    render(<DependencyNotices prOwner="acme" view={mockDependenciesNotRust()} links={links()} />);
    expect(document.body).toHaveTextContent(
      "Dependencies: not available — this view reads Cargo (Rust) workspaces only; nothing was checked.",
    );
    expect(document.body).not.toHaveTextContent(/no dependencies/i);
    expectTrustWording(document.body.textContent ?? "");
  });

  it("limits: every limit is listed and the map is called incomplete, never complete", () => {
    render(<DependencyNotices prOwner="acme" view={mockDependenciesLimits()} links={links()} />);
    const limits = screen.getByRole("region", { name: "This map is incomplete" });
    const items = within(limits)
      .getAllByRole("listitem")
      .map((li) => li.textContent);
    expect(items).toEqual([
      expect.stringContaining("400 of 512 Cargo.toml files"),
      expect.stringContaining("crates/broken/Cargo.toml"),
      expect.stringContaining("tools/Cargo.lock"),
      expect.stringMatching(/No Cargo\.lock in the workspace tools.*versions are not resolved/),
      expect.stringMatching(/No Cargo\.lock in the workspace at the repository root/),
      expect.stringMatching(/src\/rpc\/generated_codes\.rs.*conversions are not checked/),
      expect.stringContaining("first 200 conversion facts"),
      expect.stringContaining("tools/xtask/Cargo.toml: its workspace is not determined"),
      expect.stringContaining("Cargo.toml has target-specific dependency tables"),
      expect.stringMatching(/src\/rpc\/generated_codes\.rs has more conversion candidates/),
      expect.stringContaining("2 files of the crate in crates/node-sdk"),
      expect.stringContaining("Cargo.toml lists more workspace members or exclusions than are read"),
    ]);
    expect(document.body).not.toHaveTextContent(/\bcomplete\b/);
    expectTrustWording(document.body.textContent ?? "");
  });

  it("without limits no limits region and no claim of completeness", () => {
    render(<DependencyNotices prOwner="acme" view={mockDependencies()} links={links()} />);
    expect(screen.queryByRole("region", { name: "This map is incomplete" })).toBeNull();
    expect(document.body).not.toHaveTextContent(/\bcomplete\b/);
  });

  it("files outside crates are counted as not checked", () => {
    render(<DependencyNotices prOwner="acme" view={mockDependenciesMulti()} links={links()} />);
    expect(document.body).toHaveTextContent("2 changed files are outside any Cargo crate (not checked).");
  });

  it("no changed crate: says so with the files outside crates not checked", () => {
    render(<DependencyNotices prOwner="acme" view={mockDependenciesNoChangedCrate()} links={links()} />);
    expect(document.body).toHaveTextContent(
      "No Rust crate is changed by this pull request; 3 changed files outside crates are not checked.",
    );
  });

  it.each([
    [{ kind: "manifests_cut", total: 512, read: 400 }],
    [{ kind: "manifest_unreadable", path: "crates/broken/Cargo.toml" }],
  ] as const)(
    "no changed crate in a cut map (%o) says only that no crate that was read is changed",
    (limit) => {
      render(
        <DependencyNotices
          prOwner="acme"
          view={{ ...mockDependenciesNoChangedCrate(), limits: [limit] }}
          links={links()}
        />,
      );
      expect(document.body).toHaveTextContent(
        "No crate that was read is changed by this pull request; 3 changed files are in no read crate and are not checked.",
      );
      expect(document.body).not.toHaveTextContent("No Rust crate is changed");
    },
  );

  const LIMITS: DependencyView["limits"] = [
    { kind: "manifests_cut", total: 512, read: 400 },
    { kind: "manifest_unreadable", path: "crates/broken/Cargo.toml" },
    { kind: "lock_unreadable", path: "Cargo.lock" },
    { kind: "no_lock_file", workspace_dir: "" },
    { kind: "source_unread", path: "src/rpc/client.rs" },
    { kind: "facts_cut", kept: 200 },
    { kind: "workspace_unresolved", manifest: "tools/x/Cargo.toml" },
    { kind: "target_specific", manifest: "Cargo.toml" },
    { kind: "source_cut", path: "src/huge.rs" },
    { kind: "definitions_unread", dir: "crates/core", files: 3 },
    { kind: "workspace_entries_cut", manifest: "Cargo.toml" },
  ];

  it.each([
    [
      { kind: "workspace_unresolved", manifest: "tools/x/Cargo.toml" },
      "tools/x/Cargo.toml: its workspace is not determined, so its lock file and inherited dependencies are not resolved.",
    ],
    [
      { kind: "target_specific", manifest: "Cargo.toml" },
      "Cargo.toml has target-specific dependency tables; they are listed for every target (cfg is not evaluated).",
    ],
    [
      { kind: "workspace_entries_cut", manifest: "Cargo.toml" },
      "Cargo.toml lists more workspace members or exclusions than are read; crate membership may be incomplete.",
    ],
    [
      { kind: "source_cut", path: "src/huge.rs" },
      "src/huge.rs has more conversion candidates than are checked; the rest are not checked.",
    ],
    [
      { kind: "definitions_unread", dir: "", files: 1 },
      "1 file of the crate in the repository root that may define a searched type was not read; those definitions are not searched.",
    ],
  ] as [DependencyView["limits"][number], string][])(
    "limit %j is named in the incomplete-map notice",
    (limit, text) => {
      render(
        <DependencyNotices
          prOwner="acme"
          view={{ ...mockDependenciesMulti(), limits: [limit] }}
          links={links()}
        />,
      );
      expect(screen.getByRole("region", { name: "This map is incomplete" })).toHaveTextContent(text);
    },
  );

  it.each(LIMITS.map((l) => [l.kind, l] as const))(
    "available with no repositories and %s: not checked, never an absence claim; the PR owner stays implicit",
    (_, limit) => {
      const view: DependencyView = {
        ...mockDependenciesNoChangedCrate(),
        controlled_owners: ["acme"],
        repositories: [],
        files_outside_crates: 4,
        limits: [limit],
      };
      render(<DependencyNotices prOwner="acme" view={view} links={links()} />);
      expect(document.body).toHaveTextContent(
        "No crate that was read is changed by this pull request; 4 changed files are in no read crate and are not checked.",
      );
      expect(document.body).not.toHaveTextContent("No Rust crate is changed");
      expect(document.body).toHaveTextContent(
        "Yours: acme (this pull request's owner). No other organisation is listed in GitHub sync.",
      );
      expect(screen.getByRole("region", { name: "This map is incomplete" })).toBeVisible();
      expectTrustWording(document.body.textContent ?? "");
    },
  );

  it("controlled owners: the pull request's owner is always yours; the Settings list is linked in-app", async () => {
    const onNavigate = vi.fn();
    render(
      <DependencyNotices prOwner="acme" view={mockDependenciesNoControlled()} links={links(onNavigate)} />,
    );
    expect(document.body).toHaveTextContent(
      "Yours: acme (this pull request's owner). No other organisation is listed in GitHub sync.",
    );
    const settings = screen.getByRole("link", { name: "Controlled organisations" });
    expect(settings).toHaveAttribute("href", "/settings");
    expect(settings).toHaveAccessibleDescription(/Settings/);
    await userEvent.click(settings);
    expect(onNavigate).toHaveBeenCalledWith("/settings");
  });

  it("controlled owners from Settings are listed after the pull request's owner", () => {
    render(<DependencyNotices prOwner="acme" view={mockDependenciesMulti()} links={links()} />);
    expect(document.body).toHaveTextContent("Yours: acme (this pull request's owner), dashpay.");
  });

  it.each([
    ["pending", "LLM notes are not written yet: they come with the plan of this analysis."],
    ["not_produced", "LLM notes were not produced for this analysis."],
  ] as const)("notes %s: one line for the whole view", (notes, text) => {
    render(<DependencyNotices prOwner="acme" view={mockDependenciesNotes(notes)} links={links()} />);
    expect(document.body).toHaveTextContent(text);
  });
});

describe("DependencyTree (repository → crate → facts and notes, disclosure buttons)", () => {
  it("repositories in order as disclosure buttons in nested lists, not an ARIA tree; this repository open", () => {
    render(<Tree view={mockDependenciesMulti()} />);
    expect(screen.queryByRole("tree")).toBeNull();
    const list = screen.getByRole("list", { name: "Repositories" });
    const rows = within(list)
      .getAllByRole("button", { name: /^(acme|dashpay|crates\.io|\.\.\/vendored)/ })
      .map((b) => [b.textContent, b.getAttribute("aria-expanded")]);
    expect(rows).toEqual([
      [expect.stringMatching(/^acme\/node-client.*this repository.*2 crates/), "true"],
      [expect.stringMatching(/^acme\/node-sdk.*1 crate$/), "false"],
      [expect.stringMatching(/^dashpay\/platform.*1 crate$/), "false"],
      [expect.stringMatching(/^crates\.io.*1 crate$/), "false"],
      [expect.stringMatching(/^\.\.\/vendored\/hdrhist.*1 crate$/), "false"],
    ]);
  });

  it('controlled repositories carry a "yours" chip naming the Settings list; others none', () => {
    render(<Tree view={mockDependenciesMulti()} />);
    const chips = screen.getAllByText("yours");
    expect(chips).toHaveLength(3);
    expect(chips[0]).toHaveAccessibleDescription(/Controlled owners in GitHub sync/);
    const registry = screen.getByRole("button", { name: /^crates\.io/ }).closest("li")!;
    expect(within(registry).queryByText("yours")).toBeNull();
  });

  it("registry and path sources say what was not looked up or read", () => {
    render(<Tree view={mockDependenciesMulti()} />);
    expect(screen.getByRole("list", { name: "Repositories" })).toHaveTextContent(
      "crates.io — repository not looked up",
    );
    expect(screen.getByRole("list", { name: "Repositories" })).toHaveTextContent(
      "../vendored/hdrhist — path not read",
    );
  });

  it("toggles a level by click and by keyboard, and reveals its crates only when open", async () => {
    render(<Tree view={mockDependenciesMulti()} />);
    const sdk = screen.getByRole("button", { name: /^acme\/node-sdk/ });
    expect(screen.queryByRole("button", { name: /^rpc-types/ })).toBeNull();
    await userEvent.click(sdk);
    expect(sdk).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: /^rpc-types/ })).toBeVisible();
    sdk.focus();
    await userEvent.keyboard("{Enter}");
    expect(sdk).toHaveAttribute("aria-expanded", "false");
    await userEvent.keyboard(" ");
    expect(sdk).toHaveAttribute("aria-expanded", "true");
  });

  it("keys are stable per repository and crate", () => {
    const view = mockDependenciesMulti();
    const [own, sdk] = view.repositories;
    expect(repositoryKey(own!, 0)).toBe("this");
    expect(repositoryKey(sdk!, 1)).toBe("github:acme/node-sdk");
    expect(crateKey("github:acme/node-sdk", sdk!.crates[0]!)).toBe("github:acme/node-sdk/rpc-types@0.3.1");
    expect(repositoryKey(view.repositories[3]!, 3)).toBe("registry:crates.io");
  });

  it("two versions of one crate and sources without name keep separate levels", async () => {
    const view = mockDependenciesMulti();
    const registry = view.repositories[3]!;
    const syn = { ...registry.crates[0]!, name: "syn", version: "1.0.109" };
    registry.crates = [syn, { ...syn, version: "2.0.87" }];
    const unknown = {
      kind: "unknown" as const,
      repo: null,
      location: null,
      controlled: false,
      crates: [syn],
    };
    view.repositories.push(unknown, { ...unknown });
    expect(new Set(view.repositories.map((r, i) => repositoryKey(r, i))).size).toBe(view.repositories.length);
    expect(crateKey("registry:crates.io", registry.crates[0]!)).not.toBe(
      crateKey("registry:crates.io", registry.crates[1]!),
    );
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    render(<Tree view={view} />);
    await userEvent.click(screen.getByRole("button", { name: /^crates\.io/ }));
    const [v1, v2] = screen.getAllByRole("button", { name: /^syn/ });
    await userEvent.click(v1!);
    expect(v1).toHaveAttribute("aria-expanded", "true");
    expect(v2).toHaveAttribute("aria-expanded", "false");
    expect(errors).not.toHaveBeenCalled();
    errors.mockRestore();
  });

  it("onToggle receives the level's key and its new state", async () => {
    const onToggle = vi.fn();
    render(
      <DependencyTree
        view={mockDependenciesMulti()}
        open={{}}
        onToggle={onToggle}
        links={links()}
        showUnchanged
        targetCrate={null}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: /^acme\/node-client/ }));
    expect(onToggle).toHaveBeenCalledWith("this", false);
    await userEvent.click(screen.getByRole("button", { name: /^node-client/ }));
    expect(onToggle).toHaveBeenCalledWith("this/node-client@0.7.0", false);
  });

  it("a changed crate (open by default) says its changed lines and flows; flows link to the Flows view", async () => {
    const onNavigate = vi.fn();
    render(<Tree view={mockDependenciesMulti()} onNavigate={onNavigate} />);
    const crate = screen.getByRole("button", { name: /^node-client/ });
    expect(crate).toHaveAttribute("aria-expanded", "true");
    expect(crate).toHaveTextContent("212 lines changed in 9 files");
    const flow = screen.getByRole("link", { name: "Send an RPC request with retry" });
    expect(flow).toHaveAttribute("href", "/flows#f1");
    await userEvent.click(flow);
    expect(onNavigate).toHaveBeenCalledWith("/flows#f1");
    expect(screen.getByText(/^5 other direct dependencies are not listed/)).toBeVisible();
  });

  it("an unchanged crate shows who uses it, its version, revision and the Cargo.lock line (deterministic)", async () => {
    render(<Tree view={mockDependenciesMulti()} />);
    await userEvent.click(screen.getByRole("button", { name: /^acme\/node-sdk/ }));
    const crate = screen.getByRole("button", { name: /^rpc-types/ });
    expect(crate).toHaveTextContent("rpc-types 0.3.1");
    expect(crate.closest("li")).toHaveTextContent("Used by node-client");
    expect(crate).toHaveTextContent("1 conversion added for its types");
    await userEvent.click(crate);
    const row = crate.closest("li")!;
    const lock = within(row).getByRole("link", { name: "Cargo.lock:30" });
    expect(lock).toHaveAttribute("href", "https://github.com/acme/node-client/blob/HEAD/Cargo.lock#L30");
    expect(lock).toHaveAttribute("target", "_blank");
    expect(within(row).getByText("revision 4f2a9c1")).toHaveAccessibleDescription(/4f2a9c1e8b7d/);
    expect(within(row).getByText("Cargo.lock")).toHaveAccessibleDescription(/Deterministic/);
  });

  it("a crate of this repository names its manifest; a manifest declaration is labelled Cargo.toml", async () => {
    render(<Tree view={mockDependenciesMulti()} />);
    const proto = screen.getByRole("button", { name: /^node-proto/ });
    await userEvent.click(proto);
    expect(
      within(proto.closest("li")!).getByRole("link", { name: "crates/node-proto/Cargo.toml" }),
    ).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: /^\.\.\/vendored\/hdrhist/ }));
    const hdr = screen.getByRole("button", { name: /^hdrhist/ });
    await userEvent.click(hdr);
    expect(within(hdr.closest("li")!).getByText("Cargo.toml")).toHaveAccessibleDescription(/Deterministic/);
  });

  it("a fact names the repository of its type's crate and whether it is yours", () => {
    render(<Tree view={mockDependenciesNoControlled()} />);
    const facts = screen.getByRole("heading", {
      name: "node-client: Conversions added for other crates' types",
    }).parentElement!;
    const items = within(facts)
      .getAllByRole("listitem")
      .map((li) => li.firstElementChild!.textContent);
    expect(items).toEqual([
      expect.stringMatching(/from rpc-types in acme\/node-sdk \(yours\)/),
      expect.stringMatching(/from node-proto in this repository/),
      expect.stringMatching(/from node-proto in this repository/),
    ]);
  });

  it("headings: one h2 for the tree; per-crate h3s name their crate", () => {
    render(<Tree view={mockDependenciesMulti()} />);
    expect(screen.getByRole("heading", { level: 2, name: "Repositories" })).toBeVisible();
    expect(screen.getByRole("list", { name: "Repositories" })).toBeVisible();
    expect(screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual([
      "node-client: Conversions added for other crates' types",
      "node-client: LLM notes on what changed",
    ]);
  });

  it("no repositories: one line says no crate is listed and nothing here was checked, no empty list", () => {
    render(<Tree view={{ ...mockDependenciesLimits(), repositories: [] }} />);
    expect(screen.getByRole("heading", { level: 2, name: "Repositories" })).toBeVisible();
    expect(screen.queryByRole("list", { name: "Repositories" })).toBeNull();
    expect(document.body).toHaveTextContent(
      "No crate is listed: the map could not be read completely (see above); nothing here was checked.",
    );
  });

  it("states and the whole tree keep the trust wording", async () => {
    render(<Tree view={mockDependenciesLimits()} />);
    for (const button of screen.getAllByRole("button", { expanded: false })) await userEvent.click(button);
    expectTrustWording(document.body.textContent ?? "");
    expect(document.body).not.toHaveTextContent(/should (move|live)|belongs? in/i);
  });
});

describe("CrateFacts (deterministic conversion facts, never a verdict)", () => {
  it("each fact names what was added, the type and its crate, with line links and a Rust tokens badge", async () => {
    const onNavigate = vi.fn();
    render(<CrateFacts changes={changesOf(mockDependenciesMulti())} links={links(onNavigate)} />);
    const items = screen.getAllByRole("listitem");
    expect(items.map((li) => li.textContent)).toEqual([
      expect.stringMatching(/^From impl.*rpc_types::Status.*from rpc-types/),
      expect.stringMatching(/^TryFrom impl.*TransitionType.*from dpp/),
      expect.stringMatching(/^conversion function.*node_proto::NodeEvent.*from node-proto/),
      expect.stringMatching(/^impl on its type.*node_proto::Peer.*from node-proto/),
    ]);
    const first = items[0]!;
    expect(within(first).getByText("Rust tokens")).toHaveAccessibleDescription(/^Deterministic/);
    const at = within(first).getByRole("link", { name: "src/rpc/error.rs:12" });
    expect(at).toHaveAttribute("href", "/files#src/rpc/error.rs:head:12");
    expect(at).not.toHaveAttribute("target");
    await userEvent.click(at);
    expect(onNavigate).toHaveBeenCalledWith("/files#src/rpc/error.rs:head:12");
    expect(within(first).getByRole("link", { name: "src/rpc/error.rs:3" })).toBeVisible();
    expect(first).toHaveTextContent("defined in another repository (not looked up)");
  });

  it("definition: located links to the line; not located and not searched say so", () => {
    render(<CrateFacts changes={changesOf(mockDependenciesMulti())} links={links()} />);
    const [, , located, notLocated] = screen.getAllByRole("listitem");
    const def = within(located!).getByRole("link", { name: "crates/node-proto/src/event.rs:8" });
    expect(def).toHaveAttribute("target", "_blank");
    expect(def).toHaveAccessibleName("crates/node-proto/src/event.rs:8");
    expect(located).toHaveTextContent("no use line (path written in full)");
    expect(notLocated).toHaveTextContent(
      "definition not found in node-proto (generated, a macro or a re-export)",
    );
    const changes: CrateChanges = {
      ...changesOf(mockDependenciesMulti()),
      facts: [{ ...changesOf(mockDependenciesMulti()).facts[0]!, definition: { kind: "not_searched" } }],
    };
    render(<CrateFacts changes={changes} links={links()} />);
    expect(document.body).toHaveTextContent("definition not searched");
  });

  it("always shows what the token rules cover and that no finding is not evidence of absence", () => {
    render(<CrateFacts changes={changesOf(mockDependencies())} links={links()} />);
    expect(document.body).toHaveTextContent(
      "Found by token rules for From/TryFrom impls, impls on another crate's type and parse_/validate_/… functions. Macros, glob imports and re-exports are not followed. No finding here is not evidence that none exist.",
    );
    expect(document.body).toHaveTextContent("None found for types of this repository's or your crates.");
  });

  it("counts facts not listed: beyond the limit and about other crates' types", () => {
    const changes = { ...changesOf(mockDependenciesMulti()), facts_hidden: 7 };
    render(<CrateFacts changes={changes} links={links()} />);
    expect(document.body).toHaveTextContent("7 more facts are not listed (limit 50).");
    expect(document.body).toHaveTextContent(
      "2 conversions for types of other crates are counted, not listed.",
    );
    expectTrustWording(document.body.textContent ?? "");
  });
});

describe("CrateNotes (LLM keyword notes, labelled, top 3 and +N more)", () => {
  it("produced: each note is labelled LLM, renders code spans and links its lines; +N more", async () => {
    const onNavigate = vi.fn();
    render(<CrateNotes changes={changesOf(mockDependencies())} notes="produced" links={links(onNavigate)} />);
    const [note] = screen.getAllByRole("listitem");
    expect(note).toHaveTextContent("retry with backoff around RpcClient::send");
    expect(note!.querySelector("code")).toHaveTextContent("RpcClient::send");
    expect(note!.querySelector("[data-provenance=llm]")).not.toBeNull();
    const lines = within(note!).getByRole("link", { name: "src/rpc/client.rs:40–88" });
    expect(lines).toHaveAttribute("href", "/files#src/rpc/client.rs:head:40-88");
    await userEvent.click(lines);
    expect(onNavigate).toHaveBeenCalledWith("/files#src/rpc/client.rs:head:40-88");
    expect(document.body).toHaveTextContent("+3 more notes, not listed.");
    expect(document.body.querySelector("[data-badge]")).toBeNull();
  });

  it("markup in a note stays text (threat model T12)", () => {
    const changes = changesOf(mockDependencies());
    const note = { ...changes.notes[0]!, text: '<img src=x onerror="alert(1)"> `x`' };
    render(<CrateNotes changes={{ ...changes, notes: [note] }} notes="produced" links={links()} />);
    expect(document.querySelector("img")).toBeNull();
    expect(document.body).toHaveTextContent('<img src=x onerror="alert(1)">');
  });

  it.each([
    ["produced", "The LLM wrote no notes for this crate."],
    ["pending", "LLM notes are not written yet."],
    ["not_produced", "LLM notes were not produced for this analysis."],
  ] as const)("no notes, %s", (state, text) => {
    render(<CrateNotes changes={changesOf(mockDependenciesNotes(state))} notes={state} links={links()} />);
    expect(document.body).toHaveTextContent(text);
    expect(screen.queryByRole("listitem")).toBeNull();
  });
});
