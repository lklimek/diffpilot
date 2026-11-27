import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { LlmSelectionCard } from "./LlmSelectionCard";

const selection = {
  models: [
    { id: "gpt-6.1-sol", label: "Sol 6.1" },
    { id: "gpt-6-astra", label: "Astra 6" },
  ],
  efforts: ["low", "medium", "high"] as const,
  current: { model: "gpt-6.1-sol", reasoning_effort: "high" as const },
  changed_by: "alice",
  changed_at: "2026-11-17T18:04:00Z",
};
it("saves the selected installation model and effort and names the editor", async () => {
  const save = vi.fn();
  render(<LlmSelectionCard selection={{ ...selection, efforts: [...selection.efforts] }} onSave={save} />);
  await userEvent.selectOptions(screen.getByLabelText("Model"), "gpt-6-astra");
  await userEvent.selectOptions(screen.getByLabelText("Reasoning effort"), "low");
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(save).toHaveBeenCalledWith({ model: { model: "gpt-6-astra", reasoning_effort: "low" } });
  expect(screen.getByText(/changed by alice/)).toBeVisible();
});
it("keeps a pending save read-only and sends only the changed effort", async () => {
  const save = vi.fn(() => new Promise<void>(() => {}));
  render(<LlmSelectionCard selection={{ ...selection, efforts: [...selection.efforts] }} onSave={save} />);
  expect(screen.getByLabelText("Ask effort")).toHaveValue("medium");
  expect(screen.getByLabelText("Dashboard ranking effort")).toHaveValue("low");
  await userEvent.selectOptions(screen.getByLabelText("Ask effort"), "low");
  expect(save).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(save).toHaveBeenCalledExactlyOnceWith({ ask: "low" });
  expect(screen.getByLabelText("Model")).toBeDisabled();
  expect(screen.getByRole("button", { name: "Saving…" })).toHaveAttribute("aria-disabled", "true");
});
