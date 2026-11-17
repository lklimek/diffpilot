import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { DiffResponse } from "../../api/generated/types";
import { ClassicDiff } from "./ClassicDiff";
import type { Highlight } from "./highlight";

const diff: DiffResponse = {
  path: "src/rpc/client.rs",
  side: "head",
  hunks: [
    {
      header: "@@ -54,1 +97,2 @@ impl RpcClient",
      lines: [
        { kind: "context", old_line: 53, new_line: 96, text: "    let timeout = t;" },
        { kind: "removed", old_line: 54, new_line: null, text: "    send(http).await?;" },
        { kind: "added", old_line: null, new_line: 97, text: "    match send(http).await {" },
      ],
    },
  ],
};

describe("ClassicDiff", () => {
  it("shows each hunk with its header and old / new line numbers, removed and added lines named", () => {
    render(<ClassicDiff diff={diff} />);
    const region = screen.getByRole("region", { name: "Classic diff of src/rpc/client.rs" });
    expect(region).toHaveTextContent("@@ -54,1 +97,2 @@ impl RpcClient");
    const rows = [...region.querySelectorAll("[data-diff-line]")];
    expect(rows.map((r) => r.getAttribute("data-diff-line"))).toEqual(["context", "removed", "added"]);
    expect(rows.map((r) => r.textContent)).toEqual([
      "5396 unchanged    let timeout = t;",
      "54−removed    send(http).await?;",
      "97+added    match send(http).await {",
    ]);
  });

  it("says so when the range has no change", () => {
    render(<ClassicDiff diff={{ ...diff, hunks: [] }} />);
    expect(screen.getByText("No change in this range.")).toBeInTheDocument();
  });

  it("colours the lines of each hunk with one highlight call", async () => {
    const highlight = vi.fn<Highlight>(async (_, lines) =>
      lines.map((l) => [{ content: l, light: "#111111" }]),
    );
    render(<ClassicDiff diff={diff} highlight={highlight} />);
    await act(async () => {});
    expect(highlight).toHaveBeenCalledWith(
      "src/rpc/client.rs",
      diff.hunks[0]!.lines.map((l) => l.text),
      expect.any(AbortSignal),
    );
    expect(document.querySelectorAll("[data-token]")).toHaveLength(3);
  });
});
