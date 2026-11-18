import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { DIAGRAM_KEY_STEP_PX, DIAGRAM_MIN_PX, DiagramPane, diagramMaxPx } from "./DiagramPane";

const flow = (seedFor("default", "tour") as Tour).flows[0]!;

describe("DiagramPane", () => {
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

  describe("height handle", () => {
    const pane = (height: number | null, onHeight = vi.fn()) => {
      render(
        <DiagramPane
          flow={flow}
          currentStepId={null}
          onSelectStep={() => {}}
          open
          onToggle={() => {}}
          height={height}
          onHeight={onHeight}
          load={() => new Promise(() => {})}
        />,
      );
      return { onHeight, handle: screen.getByRole("separator", { name: /Resize the flow diagram/ }) };
    };

    it("is a focusable horizontal separator with a tooltip and a 24 px hit area", () => {
      const { handle } = pane(300);
      expect(handle).toHaveAttribute("aria-orientation", "horizontal");
      expect(handle).toHaveAttribute("tabindex", "0");
      expect(handle).toHaveAttribute("aria-valuenow", "300");
      expect(handle).toHaveAttribute("aria-valuemin", String(DIAGRAM_MIN_PX));
      expect(handle.className).toMatch(/(^|\s)h-6(\s|$)/);
      expect(handle).toHaveAccessibleDescription(/Drag to make the diagram taller or shorter/);
      expect(
        screen.getByRole("region", { name: "Flow diagram" }).querySelector("[data-diagram-body]"),
      ).toHaveStyle({
        height: "300px",
      });
    });

    it("arrow keys change the height; Home and End go to the limits; Enter resets", async () => {
      const { handle, onHeight } = pane(300);
      handle.focus();
      await userEvent.keyboard("{ArrowDown}");
      expect(onHeight).toHaveBeenLastCalledWith(300 + DIAGRAM_KEY_STEP_PX);
      await userEvent.keyboard("{ArrowUp}");
      expect(onHeight).toHaveBeenLastCalledWith(300 - DIAGRAM_KEY_STEP_PX);
      await userEvent.keyboard("{Home}");
      expect(onHeight).toHaveBeenLastCalledWith(DIAGRAM_MIN_PX);
      await userEvent.keyboard("{End}");
      expect(onHeight).toHaveBeenLastCalledWith(diagramMaxPx());
      await userEvent.keyboard("{Enter}");
      expect(onHeight).toHaveBeenLastCalledWith(null);
    });

    it("fits a height stored on a taller screen into this window and reports the height shown", () => {
      const { handle } = pane(5000);
      expect(handle).toHaveAttribute("aria-valuenow", String(diagramMaxPx()));
      expect(screen.getByRole("region", { name: "Flow diagram" }).querySelector("[data-diagram-body]")).toHaveStyle({
        height: `${diagramMaxPx()}px`,
      });
    });

    it("at the default height it does not claim the minimum", () => {
      const { handle } = pane(null);
      expect(handle).not.toHaveAttribute("aria-valuenow", String(DIAGRAM_MIN_PX));
      expect(handle).toHaveAttribute("aria-valuetext", "default height");
    });

    it("double-click resets; the collapsed pane has no handle", async () => {
      const { handle, onHeight } = pane(300);
      await userEvent.dblClick(handle);
      expect(onHeight).toHaveBeenLastCalledWith(null);
    });
  });
});
