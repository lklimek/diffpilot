import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { ModelsUsed } from "./ModelsUsed";
it("shows the choices actually used and explains their source", () => {
  render(
    <ModelsUsed
      models={[
        { model: "gpt-6.1-sol", reasoning_effort: "high" },
        { model: "gpt-6-astra", reasoning_effort: "low" },
      ]}
    />,
  );
  expect(
    screen.getByText("Analysed with gpt-6.1-sol, effort high; gpt-6-astra, effort low"),
  ).toHaveAccessibleDescription(/job specifications.*Ask and repair/);
});
it("has no model fact before a job has started", () => {
  const { container } = render(<ModelsUsed models={[]} />);
  expect(container).toBeEmptyDOMElement();
});
