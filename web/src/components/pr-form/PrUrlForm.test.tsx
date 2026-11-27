import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { PrUrlForm } from "./PrUrlForm";

it("submits the typed PR reference by keyboard", async () => {
  const onOpen = vi.fn(() => true);
  render(<PrUrlForm onOpen={onOpen} />);
  await userEvent.type(screen.getByLabelText("GitHub pull request URL"), "acme/node-client#482{Enter}");
  expect(onOpen).toHaveBeenCalledWith("acme/node-client#482");
});

it("keeps an invalid value, describes the error and clears it when edited", async () => {
  render(<PrUrlForm initialUrl="broken" onOpen={() => false} />);
  await userEvent.click(screen.getByRole("button", { name: "Open" }));
  const input = screen.getByLabelText("GitHub pull request URL");
  expect(input).toHaveFocus();
  expect(input).toHaveValue("broken");
  expect(input).toHaveAccessibleDescription(/owner\/repo#123/);
  expect(screen.getByRole("alert")).toBeVisible();
  await userEvent.type(input, "!");
  expect(input).not.toHaveAttribute("aria-invalid");
  expect(screen.queryByRole("alert")).toBeNull();
});
