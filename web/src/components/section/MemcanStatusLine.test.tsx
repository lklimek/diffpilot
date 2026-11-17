import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { MemcanStatusLine } from "./MemcanStatusLine";
it.each(["reachable", "not_reachable", "not_configured", "not_checked"] as const)(
  "renders observed MemCan state %s with the last success",
  (state) => {
    render(
      <p>
        <MemcanStatusLine status={{ state, last_success_at: "2026-11-17T10:00:00Z" }} />
      </p>,
    );
    expect(screen.getByText(/MemCan:/)).toHaveTextContent(
      `MemCan: ${state.replaceAll("_", " ")}; last success 17 Nov 2026, 10:00`,
    );
  },
);
