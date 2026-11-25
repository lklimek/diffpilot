import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ITEMS } from "../test-fixtures";
import { ItemAction } from "./ItemAction";

const pr = ITEMS.find((item) => item.id === "NC-482")!;
const href = "/pr/acme/node-client/482/comments";

describe("ItemAction Comments", () => {
  it.each([null, pr.review])(
    "opens Comments by keyboard, with or without an analysis (%j)",
    async (review) => {
      const go = vi.fn();
      render(
        <ItemAction item={{ ...pr, review }} reviewHref="/review" commentsHref={href} onNavigate={go} />,
      );
      const link = screen.getByRole("link", { name: "Comments of NC-482" });
      expect(link).toHaveAttribute("href", href);
      expect(link).toHaveTextContent("Comments");
      await userEvent.tab();
      await userEvent.tab();
      expect(link).toHaveFocus();
      expect(screen.getByRole("tooltip")).toHaveTextContent(/reads.*GitHub.*unless.*current read/i);
      expect(link).toHaveAccessibleDescription(/assesses.*LLM.*GitHub requests and LLM tokens/i);
      await userEvent.keyboard("{Enter}");
      expect(go).toHaveBeenCalledWith(href);
    },
  );

  it("retains native modified-click navigation", async () => {
    const go = vi.fn();
    render(<ItemAction item={pr} commentsHref={href} onNavigate={go} />);
    fireEvent.click(screen.getByRole("link", { name: "Comments of NC-482" }), { ctrlKey: true });
    expect(go).not.toHaveBeenCalled();
  });

  it.each(ITEMS.filter((item) => item.kind !== "pr"))("does not offer Comments for $kind", (item) => {
    render(<ItemAction item={item} reviewHref="/review" commentsHref={href} />);
    expect(screen.queryByRole("link", { name: /^Comments of / })).toBeNull();
  });
});
