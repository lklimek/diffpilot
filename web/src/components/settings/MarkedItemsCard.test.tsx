import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import type { MarkedItem } from "../../api/generated/types";
import { MarkedItemsCard } from "./MarkedItemsCard";

const item: MarkedItem = {
  pr: { repo: { owner: "acme", name: "node-client" }, number: 482 },
  no: 30,
  kind: "thread",
  author: "reviewer",
  author_kind: "human",
  title: "Review fixture",
  url: null,
  by: "alice",
  at: "2026-11-18T09:20:00Z",
  status_comment: false,
};

it("announces loading and an empty result", () => {
  const { rerender } = render(<MarkedItemsCard loadingOlder={false} />);
  expect(screen.getByRole("status")).toHaveTextContent("Loading marked items…");
  rerender(<MarkedItemsCard items={[]} loadingOlder={false} />);
  expect(screen.getByRole("status")).toHaveTextContent("No items marked.");
});

it("retains the current page when an older page fails and guards concurrent requests", async () => {
  const props = { items: [item], onShowOlder: vi.fn() };
  const { rerender } = render(<MarkedItemsCard {...props} loadingOlder />);
  const loading = screen.getByRole("button", { name: "Loading older marked items…" });
  expect(loading).toHaveAttribute("aria-disabled", "true");
  await userEvent.click(loading);
  expect(props.onShowOlder).not.toHaveBeenCalled();

  rerender(<MarkedItemsCard {...props} loadingOlder={false} error="Could not load older items." />);
  expect(screen.getByRole("alert")).toHaveTextContent("Could not load older items.");
  expect(screen.getByText(item.title)).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Show older marked items" }));
  expect(props.onShowOlder).toHaveBeenCalledTimes(1);
});
