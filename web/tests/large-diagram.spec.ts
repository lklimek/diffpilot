import { expect, test } from "./fixtures";
import { ANALYSIS_ID } from "./support";

// A 27-step flow with branches, joins, a back edge, skip edges and detached chains, on a 21:9 screen: neighbours
// are joined by one straight line, every other edge runs in a small lane beside the step column.
for (const colorScheme of ["light", "dark"] as const) {
  test(`large flow diagram, ${colorScheme}`, async ({ page }, info) => {
    await page.emulateMedia({ colorScheme });
    await page.setViewportSize({ width: 2520, height: 1080 });
    await page.goto(`/analyses/${ANALYSIS_ID}?scenario=large-diagram#f1.1`);
    const main = page.getByRole("main");
    await main.getByRole("button", { name: /Show flow diagram/ }).click();
    const diagram = main.getByRole("region", { name: "Flow diagram", exact: true });
    const svg = diagram.locator("svg[role=group]");
    await expect(svg).toHaveAttribute("aria-label", /27 steps/);

    // Taller, so more of it shows (F25).
    const edge = (await diagram.getByRole("separator", { name: "Resize the flow diagram" }).boundingBox())!;
    await page.mouse.move(edge.x + edge.width / 2, edge.y + edge.height / 2);
    await page.mouse.down();
    await page.mouse.move(edge.x + edge.width / 2, edge.y + edge.height / 2 + 400, { steps: 8 });
    await page.mouse.up();

    const shape = await svg.evaluate((element) => {
      const nodes = [...element.querySelectorAll<SVGGraphicsElement>("[data-step]")].map((n) => n.getBBox());
      const left = Math.min(...nodes.map((b) => b.x));
      const right = Math.max(...nodes.map((b) => b.x + b.width));
      const paths = [...element.querySelectorAll("[data-edge]")].map((p) =>
        [...(p.getAttribute("d") ?? "").matchAll(/[ML]([\d.-]+),([\d.-]+)/g)].map((m) => Number(m[1])),
      );
      return {
        straight: paths.filter((xs) => xs.length === 2 && xs[0] === xs[1]).length,
        outside: Math.max(...paths.flat().map((x) => Math.max(left - x, x - right))),
        width: element.getBoundingClientRect().width,
      };
    });
    // The 19 edges between neighbours in the walk are vertical lines.
    expect(shape.straight).toBe(19);
    // No bend far from the column: lanes start 32 px out and step 14 px.
    expect(shape.outside).toBeLessThanOrEqual(32 + 14 * 16);
    expect(shape.width).toBeLessThan(1000);
    await diagram.screenshot({ path: info.outputPath(`large-diagram-${colorScheme}.png`) });

    await page
      .getByRole("navigation")
      .getByRole("button", { name: /^Flow 2:/ })
      .click();
    await expect(svg).toHaveAttribute("aria-label", /12 steps/);
    await diagram.screenshot({ path: info.outputPath(`medium-diagram-${colorScheme}.png`) });
  });
}
