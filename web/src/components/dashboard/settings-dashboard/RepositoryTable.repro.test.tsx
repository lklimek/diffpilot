import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import { RepositoryTable } from "./RepositoryTable";
import type { RepositoryDraft } from "./validation";

function Harness() {
  const [rows, setRows] = useState<RepositoryDraft[]>([]);
  return (
    <RepositoryTable
      rows={rows}
      errors={{}}
      unreachable={[]}
      installations={[{ name: "acme/platform", live: true }]}
      installationStatusKnown
      onChange={setRows}
    />
  );
}

describe("RepositoryTable new row whose name passes through an installed repository", () => {
  it("keeps the repository input while typing acme/platform-tools", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole("button", { name: "Add repository" }));
    const input = screen.getByLabelText("Repository 1");
    await user.type(input, "acme/platform-tools");
    expect(screen.getByLabelText("Repository 1")).toHaveValue("acme/platform-tools");
  });

  it("typing into the short name of an installation-only row keeps focus", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const alias = screen.getByLabelText("Short name of acme/platform");
    await user.type(alias, "pl");
    expect(screen.getByLabelText("Short name of acme/platform")).toHaveValue("PL");
    expect(screen.getByLabelText("Short name of acme/platform")).toHaveFocus();
  });
});
