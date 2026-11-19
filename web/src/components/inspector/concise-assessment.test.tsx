import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import type { Assessment } from "../../api/generated/types";
import { AssessmentFields } from "./StepSections";

it("shows only the band and reason for a concise low assessment", () => {
  render(<AssessmentFields assessment={{ risk: "low", reason: "The wait is bounded." } as Assessment} />);
  expect(screen.getByText("The wait is bounded.")).toBeVisible();
  expect(screen.queryByText("Basis")).not.toBeInTheDocument();
  expect(screen.queryByText("Not checked")).not.toBeInTheDocument();
  expect(screen.queryByText("Smallest correction")).not.toBeInTheDocument();
});
