import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { AccessToken, CreatedAccessToken } from "../../../api/generated/types";
import tokensJson from "../../../../../fixtures/dashboard/api/access-tokens.json";
import createdJson from "../../../../../fixtures/dashboard/api/access-token-created.json";
import { AccessTokensCard } from "./AccessTokensCard";

const TOKENS = tokensJson as unknown as AccessToken[];
const CREATED = createdJson as unknown as CreatedAccessToken;
const SECRET = CREATED.token;
const NOW = new Date("2026-11-18T12:00:00Z");

function renderCard(props: Partial<Parameters<typeof AccessTokensCard>[0]> = {}): {
  onCreate: ReturnType<typeof vi.fn>;
  onRevoke: ReturnType<typeof vi.fn>;
} {
  const onCreate = vi.fn().mockResolvedValue(CREATED);
  const onRevoke = vi.fn().mockResolvedValue(undefined);
  render(<AccessTokensCard tokens={TOKENS} onCreate={onCreate} onRevoke={onRevoke} now={NOW} {...props} />);
  return { onCreate, onRevoke };
}

async function createToken(name = "ci agent") {
  await userEvent.click(screen.getByRole("button", { name: "New token" }));
  const dialog = screen.getByRole("dialog", { name: "New access token" });
  await userEvent.type(within(dialog).getByLabelText("Name"), name);
  await userEvent.click(within(dialog).getByRole("button", { name: "Create token" }));
}

describe("AccessTokensCard (ADR-019, plan §F.6)", () => {
  it("lists the tokens with prefix, scope, dates and says tokens are read-only", () => {
    renderCard();
    const card = screen.getByRole("region", { name: "Access tokens" });
    expect(card).toHaveTextContent("read-only");
    expect(card).toHaveTextContent("cannot change anything");
    const rows = within(screen.getByRole("table", { name: "Your access tokens" })).getAllByRole("row");
    expect(rows[1]).toHaveTextContent("dispatcher agentsrp_Ab3dE5gH…read18 Nov 2026");
    expect(rows[2]).toHaveTextContent("never");
    expect(card).toHaveTextContent("2 of 10 tokens in use.");
    expect(card).not.toHaveTextContent("expired");
  });

  it("marks an expired token and does not count it as live", () => {
    renderCard({ now: new Date("2027-01-01T00:00:00Z") });
    const row = screen.getByRole("row", { name: /laptop scripts/ });
    expect(row).toHaveTextContent("expired");
    expect(screen.getByRole("region", { name: "Access tokens" })).toHaveTextContent("1 of 10 tokens in use.");
  });

  it("creates a read token (90 days by default) and shows its secret once with a copy button", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { onCreate } = renderCard();
    await userEvent.click(screen.getByRole("button", { name: "New token" }));
    expect(screen.getByLabelText("Expires after")).toHaveValue("90");
    expect(screen.getByLabelText("Name")).toHaveFocus();
    await userEvent.type(screen.getByLabelText("Name"), "ci agent");
    await userEvent.click(screen.getByRole("button", { name: "Create token" }));
    expect(onCreate).toHaveBeenCalledWith({ name: "ci agent", scopes: ["read"], expires_in_days: 90 });
    const dialog = screen.getByRole("dialog", { name: "Token created" });
    expect(dialog).toHaveTextContent("Shown only now.");
    const secret = within(dialog).getByLabelText("Your new token");
    expect(secret).toHaveValue(SECRET);
    expect(secret).toHaveAttribute("readonly");
    expect(secret).toHaveFocus();
    await userEvent.click(within(dialog).getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(SECRET);
    expect(within(dialog).getByRole("status")).toHaveTextContent("Copied");
    await userEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(screen.getByRole("button", { name: "New token" })).toHaveFocus();
    // Opening the dialog again starts from the form: the secret is gone.
    await userEvent.click(screen.getByRole("button", { name: "New token" }));
    expect(screen.getByRole("dialog", { name: "New access token" })).not.toHaveTextContent(SECRET);
    expect(JSON.stringify({ ...localStorage, ...sessionStorage })).not.toContain(SECRET);
  });

  it("adds the review-loop execute scope only when ticked", async () => {
    const { onCreate } = renderCard();
    await userEvent.click(screen.getByRole("button", { name: "New token" }));
    await userEvent.type(screen.getByLabelText("Name"), "implementing agent");
    await userEvent.click(screen.getByRole("checkbox", { name: /review_loop_execute/ }));
    await userEvent.click(screen.getByRole("button", { name: "Create token" }));
    expect(onCreate).toHaveBeenCalledWith({
      name: "implementing agent",
      scopes: ["read", "review_loop_execute"],
      expires_in_days: 90,
    });
  });

  it("Escape closes the form, but on the secret view only Done closes", async () => {
    renderCard();
    await userEvent.click(screen.getByRole("button", { name: "New token" }));
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    await createToken();
    screen.getByRole("button", { name: "Copy" }).focus();
    await userEvent.keyboard("{Escape}");
    expect(screen.getByLabelText("Your new token")).toHaveValue(SECRET);
    await userEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(document.body.innerHTML).not.toContain(SECRET);
  });

  it("accepts a 60-character name counted in characters, not code units", async () => {
    const { onCreate } = renderCard();
    await userEvent.click(screen.getByRole("button", { name: "New token" }));
    const name = screen.getByLabelText("Name");
    await userEvent.type(name, "🔑".repeat(61));
    await userEvent.click(screen.getByRole("button", { name: "Create token" }));
    expect(onCreate).not.toHaveBeenCalled();
    await userEvent.type(name, "{Backspace}{Backspace}");
    await userEvent.click(screen.getByRole("button", { name: "Create token" }));
    expect(onCreate).toHaveBeenCalledWith(expect.objectContaining({ name: "🔑".repeat(60) }));
  });

  it("explains each column and the expired mark", () => {
    renderCard({ now: new Date("2027-01-01T00:00:00Z") });
    expect(
      screen.getByRole("columnheader", { name: "Last used" }).querySelector("[tabindex]"),
    ).toHaveAccessibleDescription(/at most once a minute/);
    expect(screen.getByText("expired")).toHaveAccessibleDescription(/stopped working/);
  });

  it("when copying fails says to copy by hand", async () => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn().mockRejectedValue(new Error("denied")) },
      configurable: true,
    });
    renderCard();
    await createToken();
    await userEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Select the token and copy it yourself.");
  });

  it("refuses an empty or repeated name before sending, and shows the server's refusal", async () => {
    const onCreate = vi
      .fn()
      .mockRejectedValue(new Error("You have 10 tokens. Revoke one to create another."));
    renderCard({ onCreate });
    await userEvent.click(screen.getByRole("button", { name: "New token" }));
    await userEvent.click(screen.getByRole("button", { name: "Create token" }));
    expect(screen.getByLabelText("Name")).toHaveAccessibleDescription(/use 1 to 60 characters/);
    await userEvent.type(screen.getByLabelText("Name"), "laptop scripts");
    await userEvent.click(screen.getByRole("button", { name: "Create token" }));
    expect(screen.getByRole("alert")).toHaveTextContent('already have a token called "laptop scripts"');
    expect(onCreate).not.toHaveBeenCalled();
    await userEvent.type(screen.getByLabelText("Name"), " 2");
    await userEvent.click(screen.getByRole("button", { name: "Create token" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Not created: You have 10 tokens.");
    expect(screen.getByRole("dialog", { name: "New access token" })).toBeVisible();
  });

  it("at 10 live tokens New token is unavailable and says why", async () => {
    const ten = Array.from({ length: 10 }, (_, i) => ({ ...TOKENS[0]!, id: `id${i}`, name: `t${i}` }));
    renderCard({ tokens: ten });
    const button = screen.getByRole("button", { name: "New token" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription("You have 10 tokens. Revoke one to create another.");
    await userEvent.click(button);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("revokes only after confirmation; focus goes to Keep, back to Revoke, then to New token", async () => {
    const { onRevoke } = renderCard();
    await userEvent.click(screen.getByRole("button", { name: "Revoke laptop scripts" }));
    const confirm = screen.getByRole("group", { name: "Confirm revoking laptop scripts" });
    expect(confirm).toHaveTextContent("Agents using it lose access at once.");
    expect(within(confirm).getByRole("button", { name: "Keep" })).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    expect(onRevoke).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Revoke laptop scripts" })).toHaveFocus();
    await userEvent.click(screen.getByRole("button", { name: "Revoke laptop scripts" }));
    await userEvent.click(screen.getByRole("button", { name: "Revoke token" }));
    expect(onRevoke).toHaveBeenCalledWith("Zq9xW2vB");
    expect(screen.getByRole("button", { name: "New token" })).toHaveFocus();
  });

  it("with no tokens says so", () => {
    renderCard({ tokens: [] });
    expect(screen.getByText("No tokens yet.")).toBeVisible();
    expect(screen.queryByRole("table")).toBeNull();
  });
});
