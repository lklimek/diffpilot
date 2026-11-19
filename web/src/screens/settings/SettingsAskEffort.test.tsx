import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import { resetSelection } from "../../mocks/handlers/set-llm";

startMockServer();
afterEach(resetSelection);

describe("Settings: Ask effort in the LLM card", () => {
  it("shows the default, saves a change and keeps the model choice", async () => {
    renderRoute("/settings");
    const llm = await screen.findByRole("region", { name: "LLM model" });
    const select = within(llm).getByLabelText("Ask effort");
    expect(select).toHaveValue("medium");
    await userEvent.selectOptions(select, "low");
    expect(await screen.findByText("Ask effort saved.")).toBeVisible();
    const card = await screen.findByRole("region", { name: "LLM model" });
    expect(within(card).getByLabelText("Ask effort")).toHaveValue("low");
    expect(within(card).getByLabelText("Reasoning effort")).toHaveValue("high");
    expect(card).toHaveTextContent(/changed by alice/);
    expectTrustWording(document.body.textContent ?? "");
  });
});
