import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Mention } from "../../../api/generated/types";
import { Mentions } from "./Mentions";

const mention = (id: string, bot = false): Mention => ({
  id,
  item_key: "acme~app~pr~1",
  repo: { owner: "acme", name: "app" },
  number: 1,
  kind: "pr",
  title: "Please look",
  author: bot ? "helper" : "bob",
  is_bot: bot,
  created_at: `2026-11-18T0${id}:00:00Z`,
  text: "<script>alert(1)</script> @alice",
  url: `https://github.com/acme/app/pull/1#issuecomment-${id}`,
});
const props = {
  now: new Date("2026-11-18T09:00:00Z"),
  withBots: false,
  onWithBotsChange: vi.fn(),
  onHide: vi.fn(),
  hiding: false,
};

describe("Mentions", () => {
  it("hides the empty block", () => {
    const { container } = render(<Mentions {...props} items={[]} />);
    expect(container).toBeEmptyDOMElement();
  });
  it("shows three oldest human mentions, expands the rest, renders text and hides one", async () => {
    const user = userEvent.setup();
    render(
      <Mentions
        {...props}
        items={[mention("4"), mention("2"), mention("3"), mention("1"), mention("5", true)]}
      />,
    );
    expect(screen.getAllByRole("link").map((a) => a.getAttribute("href"))).toEqual(
      ["1", "2", "3"].map((id) => mention(id).url),
    );
    expect(screen.getAllByText("<script>alert(1)</script> @alice")).toHaveLength(3);
    await user.click(screen.getByText("+1 more"));
    expect(screen.getAllByRole("link")).toHaveLength(4);
    await user.click(screen.getAllByRole("button", { name: /Hide mention/ })[0]!);
    expect(props.onHide).toHaveBeenCalledWith("1");
    await user.click(screen.getByRole("checkbox", { name: "with bots" }));
    expect(props.onWithBotsChange).toHaveBeenCalledWith(true);
  });
  it("keeps the toggle reachable with only bots and labels bots when shown", () => {
    const { rerender } = render(<Mentions {...props} items={[mention("1", true)]} />);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByText("without bots")).toBeInTheDocument();
    rerender(<Mentions {...props} withBots items={[mention("1", true)]} />);
    expect(screen.getByText("bot")).toBeInTheDocument();
  });
});

it("keeps a pending Hide focusable without sending another hide", async () => {
  const hide = vi.fn();
  render(<Mentions {...props} items={[mention("1")]} hiding onHide={hide} />);
  const button = screen.getByRole("button", { name: /Hide mention/ });
  expect(button).toHaveAttribute("aria-disabled", "true");
  expect(button).not.toBeDisabled();
  await userEvent.click(button);
  expect(hide).not.toHaveBeenCalled();
});
