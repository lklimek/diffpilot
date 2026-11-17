import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Feedback, FeedbackTarget } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FIXTURE_STATES, seedFor } from "../../mocks/fixtures";
import { FeedbackButtons } from "./FeedbackButtons";

const VALUES = ["correct", "wrong", "irrelevant", "overrated", "underrated"];
const step31: FeedbackTarget = { target: "step", step_id: "f3.1" };
const line96: FeedbackTarget = { target: "line", path: "src/rpc/client.rs", side: "head", line: 96 };
const feedbackOf = (state: (typeof FIXTURE_STATES)[number]) =>
  (seedFor(state, "feedback") ?? []) as Feedback[];

describe("FeedbackButtons", () => {
  it("offers the five values, each with a tooltip, none pressed without stored feedback", () => {
    render(<FeedbackButtons target={step31} subject="step 1" feedback={[]} onSubmit={() => {}} />);
    const group = screen.getByRole("group", { name: "Feedback on step 1" });
    const buttons = within(group).getAllByRole("button");
    expect(buttons.map((b) => b.textContent)).toEqual(VALUES);
    for (const b of buttons) {
      expect(b).toHaveAttribute("aria-pressed", "false");
      expect(b).toHaveAccessibleDescription(/.+/);
    }
    expect(screen.getByRole("button", { name: "overrated" })).toHaveAccessibleDescription(
      /calibration example/,
    );
  });

  it("shows the stored wrong and overrated of state wrong as pressed, with the note", () => {
    render(
      <FeedbackButtons target={step31} subject="step 1" feedback={feedbackOf("wrong")} onSubmit={() => {}} />,
    );
    expect(screen.getByRole("button", { name: "wrong" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "overrated" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "correct" })).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByRole("textbox", { name: "Optional note" })).toHaveValue(
      "RpcError is #[non_exhaustive] (src/rpc/error.rs:7); a new variant does not break downstream matches.",
    );
  });

  it("ignores feedback on other targets", () => {
    render(
      <FeedbackButtons
        target={{ target: "step", step_id: "f1.5" }}
        subject="step 5"
        feedback={feedbackOf("wrong")}
        onSubmit={() => {}}
      />,
    );
    expect(screen.getByRole("button", { name: "wrong" })).toHaveAttribute("aria-pressed", "false");
  });

  it("sends the value with the note written so far", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    render(<FeedbackButtons target={step31} subject="step 1" feedback={[]} onSubmit={onSubmit} />);
    await user.type(screen.getByRole("textbox", { name: "Optional note" }), "  non_exhaustive  ");
    await user.click(screen.getByRole("button", { name: "wrong" }));
    expect(onSubmit).toHaveBeenCalledWith({ target: step31, value: "wrong", note: "non_exhaustive" });
    expect(await screen.findByRole("status")).toHaveTextContent("Feedback saved: wrong.");
  });

  it("sends a null note when the note is empty, from the keyboard", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    render(<FeedbackButtons target={step31} subject="step 1" feedback={[]} onSubmit={onSubmit} />);
    screen.getByRole("button", { name: "correct" }).focus();
    await user.tab();
    expect(screen.getByRole("button", { name: "wrong" })).toHaveFocus();
    await user.keyboard(" ");
    expect(onSubmit).toHaveBeenCalledWith({ target: step31, value: "wrong", note: null });
  });

  it("saves a changed note on the stored value", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const stored = feedbackOf("wrong").filter((f) => f.value === "wrong");
    render(<FeedbackButtons target={step31} subject="step 1" feedback={stored} onSubmit={onSubmit} />);
    const note = screen.getByRole("textbox", { name: "Optional note" });
    await user.clear(note);
    await user.type(note, "see error.rs:7");
    await user.click(screen.getByRole("button", { name: "Save note" }));
    expect(onSubmit).toHaveBeenCalledWith({ target: step31, value: "wrong", note: "see error.rs:7" });
  });

  it("ignores a save that completes after the target changed (QA-001)", async () => {
    let resolve!: () => void;
    const onSubmit = vi.fn().mockReturnValue(new Promise<void>((r) => (resolve = r)));
    const other: FeedbackTarget = { target: "step", step_id: "f1.5" };
    const { rerender } = render(
      <FeedbackButtons target={step31} subject="step 1" feedback={[]} onSubmit={onSubmit} />,
    );
    await userEvent.click(screen.getByRole("button", { name: "wrong" }));
    rerender(<FeedbackButtons target={other} subject="step 5" feedback={[]} onSubmit={onSubmit} />);
    expect(screen.getByRole("button", { name: "correct" })).toBeEnabled();
    resolve();
    await Promise.resolve();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("keeps the note read-only while the feedback is saved (QA-001)", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn().mockReturnValue(new Promise(() => {}));
    render(<FeedbackButtons target={step31} subject="step 1" feedback={[]} onSubmit={onSubmit} />);
    const note = screen.getByRole("textbox", { name: "Optional note" });
    await user.type(note, "why");
    await user.click(screen.getByRole("button", { name: "wrong" }));
    expect(note).toHaveAttribute("readonly");
    await user.type(note, "!");
    expect(note).toHaveValue("why");
  });

  it("shows why feedback was not saved", async () => {
    const onSubmit = vi.fn().mockRejectedValue(new Error("Not signed in."));
    render(<FeedbackButtons target={step31} subject="step 1" feedback={[]} onSubmit={onSubmit} />);
    await userEvent.click(screen.getByRole("button", { name: "irrelevant" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Feedback not saved: Not signed in.");
  });

  it("names missed-by-the-LLM as recorded automatically, not as a button", () => {
    render(
      <FeedbackButtons
        target={line96}
        subject="line 96"
        feedback={feedbackOf("missed")}
        onSubmit={() => {}}
      />,
    );
    expect(screen.getByText(/Recorded automatically: missed by the LLM/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /missed/ })).toBeNull();
  });

  it("explains where feedback applies when there is no target", () => {
    render(<FeedbackButtons target={null} subject="PR context" feedback={[]} onSubmit={() => {}} />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText(/Feedback applies to a step or an annotation/)).toBeInTheDocument();
  });

  it("resets the note draft when the target changes", async () => {
    const { rerender } = render(
      <FeedbackButtons target={step31} subject="step 1" feedback={[]} onSubmit={() => {}} />,
    );
    await userEvent.type(screen.getByRole("textbox", { name: "Optional note" }), "draft");
    rerender(
      <FeedbackButtons
        target={{ target: "step", step_id: "f1.5" }}
        subject="step 5"
        feedback={[]}
        onSubmit={() => {}}
      />,
    );
    expect(screen.getByRole("textbox", { name: "Optional note" })).toHaveValue("");
  });

  it.each(FIXTURE_STATES)("keeps the wording rules in state %s", (state) => {
    const { container } = render(
      <FeedbackButtons target={step31} subject="step 1" feedback={feedbackOf(state)} onSubmit={() => {}} />,
    );
    expectTrustWording(container.textContent ?? "");
  });
});
