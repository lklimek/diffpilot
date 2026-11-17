import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { DiagramPane } from "./DiagramPane";

const flow = (seedFor("default", "tour") as Tour).flows[0]!;

describe("DiagramPane (QA-007)", () => {
  it("a failed diagram chunk leaves a text list of the steps and a retry; the rest stays", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const load = vi
      .fn()
      .mockRejectedValueOnce(new TypeError("Failed to fetch dynamically imported module"))
      .mockResolvedValue({ default: () => <p>diagram drawn</p> });
    const onSelectStep = vi.fn();
    render(
      <>
        <DiagramPane
          flow={flow}
          currentStepId={null}
          onSelectStep={onSelectStep}
          open
          onToggle={() => {}}
          load={load}
        />
        <p>the block</p>
      </>,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("The diagram could not be loaded.");
    expect(screen.getByText("the block")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /^Step 5: / }));
    expect(onSelectStep).toHaveBeenCalledWith("f1.5");
    expect(screen.getByText(/^Step 3: /).parentElement).toHaveTextContent(/→ step 4 \(.+\), step 5/);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("diagram drawn")).toBeInTheDocument();
    expect(load).toHaveBeenCalledTimes(2);
  });

  it("loads nothing while hidden", () => {
    const load = vi.fn();
    render(
      <DiagramPane
        flow={flow}
        currentStepId={null}
        onSelectStep={() => {}}
        open={false}
        onToggle={() => {}}
        load={load}
      />,
    );
    expect(load).not.toHaveBeenCalled();
  });
});
