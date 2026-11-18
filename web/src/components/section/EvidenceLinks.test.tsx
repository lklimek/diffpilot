import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { EvidenceLinks } from "./EvidenceLinks";

describe("EvidenceLinks", () => {
  it("names every file, the currently viewed one in bold", async () => {
    render(
      <EvidenceLinks
        currentFile="src/rpc/client.rs"
        evidence={[
          { file: "src/rpc/client.rs", side: "head", line: 99 },
          { file: "src/rpc/retry.rs", side: "head", line: 23 },
        ]}
      />,
    );
    await userEvent.click(screen.getByText("Evidence · 2 places"));
    const current = screen.getByText("client.rs");
    const other = screen.getByText("retry.rs");
    expect(current).toHaveClass("font-bold");
    expect(current).toHaveAccessibleDescription(/src\/rpc\/client\.rs.*current/);
    expect(other).not.toHaveClass("font-bold");
    expect(other).toHaveAccessibleDescription("src/rpc/retry.rs");
  });
});
