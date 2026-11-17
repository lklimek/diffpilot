import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Banner } from "./Banner";

describe("Banner", () => {
  it("announces errors and warnings as alerts, information as status", () => {
    render(
      <>
        <Banner tone="error" title="Bad.">
          Details
        </Banner>
        <Banner tone="warn" title="Careful." />
        <Banner tone="info" title="Found." actions={<button type="button">Go</button>} />
      </>,
    );
    const alerts = screen.getAllByRole("alert");
    expect(alerts.map((a) => a.textContent)).toEqual(["Bad.Details", "Careful."]);
    expect(screen.getByRole("status")).toHaveTextContent("Found.");
    expect(screen.getByRole("button", { name: "Go" })).toBeVisible();
  });
});
