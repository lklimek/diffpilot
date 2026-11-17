import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { PendingReview, Tour } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FIXTURE_STATES, seedFor } from "../../mocks/fixtures";
import { MovesBlock } from "./MovesBlock";
import { NotAnalysedBlock } from "./NotAnalysedBlock";
import { PendingReviewBlock } from "./PendingReviewBlock";
import { UnexplainedBlock } from "./UnexplainedBlock";

const tour = seedFor("default", "tour") as Tour;
const PR_URL = tour.analysis.pr.html_url;
const section = (t: Tour, category: string) => t.quiet.not_analysed.find((s) => s.category === category)!;

describe("UnexplainedBlock", () => {
  it("names the lines no flow explains and shows their code from the slot", () => {
    render(<UnexplainedBlock items={tour.quiet.unexplained} code={(item) => <p>code of {item.file}</p>} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Unexplained — read these yourself");
    expect(screen.getByRole("article")).toHaveTextContent(
      "6 changed lines in 1 file were not assigned to any flow",
    );
    expect(screen.getByText("src/rpc/metrics.rs")).toBeInTheDocument();
    expect(screen.getByText("lines 41–46")).toBeInTheDocument();
    expect(screen.getByText("code of src/rpc/metrics.rs")).toBeInTheDocument();
  });

  it("marks removed lines at the base commit", () => {
    render(
      <UnexplainedBlock
        items={[{ file: "src/old.rs", side: "base", ranges: [{ start_line: 3, end_line: 3 }] }]}
      />,
    );
    expect(screen.getByText("removed line 3 (base commit)")).toBeInTheDocument();
  });

  it("counts a file with base and head entries once", () => {
    render(
      <UnexplainedBlock
        items={[
          { file: "src/a.rs", side: "base", ranges: [{ start_line: 3, end_line: 3 }] },
          { file: "src/a.rs", side: "head", ranges: [{ start_line: 4, end_line: 5 }] },
        ]}
      />,
    );
    expect(screen.getByRole("article")).toHaveTextContent("3 changed lines in 1 file were not assigned");
  });

  it("says which lines were not sent to the LLM because they were too large", () => {
    render(
      <UnexplainedBlock
        items={[
          { file: "src/a.rs", side: "head", ranges: [{ start_line: 1, end_line: 2 }] },
          {
            file: "src/a.rs",
            side: "head",
            ranges: [{ start_line: 5, end_line: 9 }],
            reason: "too_large_for_prompt",
          },
        ]}
      />,
    );
    const items = screen.getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0]).not.toHaveTextContent("Not sent to the LLM");
    expect(items[1]).toHaveTextContent("lines 5–9");
    expect(items[1]).toHaveTextContent("Not sent to the LLM: too large for one request");
  });

  it("is never empty", () => {
    render(<UnexplainedBlock items={[]} />);
    expect(screen.getByRole("article")).toHaveTextContent(
      "The analysis assigned every changed line to a flow or a Not analysed section.",
    );
  });
});

describe("MovesBlock", () => {
  it("lists identical moves with the deterministic badge and says what the check does not cover", () => {
    render(<MovesBlock moves={tour.quiet.moves} code={(m) => <p>code at {m.to.range.start_line}</p>} />);
    const block = screen.getByRole("article");
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Moves with identical text");
    expect(within(block).getByText("✓ text identical")).toHaveAttribute("data-badge", "text_identical");
    expect(block).toHaveTextContent("Body text identical");
    expect(block).toHaveTextContent("Identical text does not mean unchanged behaviour");
    expect(block).toHaveTextContent("src/rpc/client.rs:112–152");
    expect(block).toHaveTextContent("Moved from src/rpc/client.rs:173–213 at the base commit; 41 lines");
    expect(block).toHaveTextContent("Moves whose text differs are steps of their flows");
    expect(screen.getByText("code at 112")).toBeInTheDocument();
  });

  it("is never empty", () => {
    render(<MovesBlock moves={[]} />);
    expect(screen.getByRole("article")).toHaveTextContent("No moved block has identical text.");
  });
});

describe("NotAnalysedBlock", () => {
  it("reads 'Recognised by file path, no LLM' with the section's files, rules and test counter", () => {
    render(<NotAnalysedBlock section={section(tour, "tests")} counters={tour.pr_context.facts.counters} />);
    const block = screen.getByRole("article");
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Tests · 4 files");
    expect(within(block).getByText("classified by path")).toHaveAttribute("data-badge");
    expect(block).toHaveTextContent("Recognised by file path, no LLM.");
    expect(block).toHaveTextContent("12 test lines changed or removed, 30 added");
    const items = within(block).getAllByRole("listitem");
    expect(items).toHaveLength(6);
    expect(block).toHaveTextContent("tests/rpc_retry.rs");
    expect(block).toHaveTextContent("whole file");
    expect(block).toHaveTextContent("tests/send_once.rs");
    expect(block).toHaveTextContent("whole file, removed (base commit)");
    expect(block).toHaveTextContent("tests/**");
  });

  it("names both rule kinds for docs and comments", () => {
    render(
      <NotAnalysedBlock section={section(tour, "docs_comments")} counters={tour.pr_context.facts.counters} />,
    );
    const block = screen.getByRole("article");
    expect(within(block).getByText("classified by path")).toBeInTheDocument();
    expect(within(block).getByText("classified by line rule")).toBeInTheDocument();
    expect(block).toHaveTextContent("Recognised by file path and line rule, no LLM.");
    expect(block).toHaveTextContent("comment-only lines (Rust, by the compiler's lexer)");
    expect(block).not.toHaveTextContent(/removed from tests|dependencies changed/);
  });

  it("states dependency files that were not checked instead of a zero count", () => {
    const counters = {
      ...tour.pr_context.facts.counters,
      dependencies_changed: [],
      dependency_files_not_checked: ["Cargo.lock"],
    };
    render(<NotAnalysedBlock section={section(tour, "generated_mechanical")} counters={counters} />);
    const block = screen.getByRole("article");
    expect(block).toHaveTextContent("Dependency files not checked: Cargo.lock");
    expect(block).not.toHaveTextContent(/0 dependencies/);
  });

  it("lists a binary file with its badge, tooltip and how it was recognised", () => {
    const mechanical = structuredClone(section(tour, "generated_mechanical"));
    mechanical.items.push({
      file: "assets/logo.png",
      side: "head",
      ranges: null,
      badge: "binary_content",
      rule: "binary file (git)",
    });
    mechanical.file_count += 1;
    render(<NotAnalysedBlock section={mechanical} counters={tour.pr_context.facts.counters} />);
    const block = screen.getByRole("article");
    const badge = within(block).getByText("binary file");
    expect(badge.closest("[data-badge]")).toHaveAttribute("data-badge", "binary_content");
    expect(badge.closest("[aria-describedby]") ?? badge).toHaveAccessibleDescription(
      "Git treats this file as binary; it is not shown or analysed.",
    );
    expect(block).toHaveTextContent("assets/logo.png whole file · rule binary file (git)");
    expect(block).toHaveTextContent("Recognised by file path and git's binary check, no LLM.");
  });

  it("names git's binary check alone for a section of binary files only", () => {
    const binaries = {
      category: "generated_mechanical" as const,
      file_count: 1,
      items: [
        {
          file: "assets/logo.png",
          side: "head" as const,
          ranges: null,
          badge: "binary_content" as const,
          rule: "binary file (git)",
        },
      ],
    };
    render(<NotAnalysedBlock section={binaries} counters={tour.pr_context.facts.counters} />);
    expect(screen.getByRole("article")).toHaveTextContent("Recognised by git's binary check, no LLM.");
  });

  it("shows the dependency counter with mechanical and generated material", () => {
    render(
      <NotAnalysedBlock
        section={section(tour, "generated_mechanical")}
        counters={tour.pr_context.facts.counters}
      />,
    );
    const block = screen.getByRole("article");
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Mechanical & generated · 1 file");
    expect(block).toHaveTextContent("3 dependencies changed: tokio, tracing, url");
    expect(block).toHaveTextContent("counted, not interpreted");
  });
});

describe("PendingReviewBlock", () => {
  const missed = seedFor("missed", "pending-review") as PendingReview;

  it("says when more pending comments exist on GitHub than are listed", () => {
    const review = { ...structuredClone(missed), incomplete: true };
    render(<PendingReviewBlock review={review} prUrl={PR_URL} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Pending review on GitHub (1+)");
    expect(screen.getByRole("article")).toHaveTextContent(
      "More pending comments exist on GitHub than are listed here.",
    );
  });

  it("lists the pending comments with their location; bodies are text, never HTML", () => {
    const review = structuredClone(missed);
    review.comments.push({
      ...review.comments[0]!,
      id: "c2",
      kind: "file",
      side: null,
      start_line: 20,
      line: 22,
      path: "src/x.rs",
      body: '<img src=x onerror="alert(1)"> nit',
    });
    const { container } = render(<PendingReviewBlock review={review} prUrl={PR_URL} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Pending review on GitHub (2)");
    const items = screen.getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("src/rpc/client.rs:96");
    expect(items[0]).toHaveTextContent(missed.comments[0]!.body);
    expect(items[1]).toHaveTextContent("src/x.rs (file comment, links to lines 20–22)");
    expect(items[1]).toHaveTextContent('<img src=x onerror="alert(1)"> nit');
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByRole("link", { name: /Finish on GitHub/ })).toHaveAttribute("href", `${PR_URL}/files`);
  });

  const deferred = () => {
    let resolve!: () => void;
    let reject!: (e: unknown) => void;
    const promise = new Promise<void>((res, rej) => {
      resolve = res;
      reject = rej;
    });
    return { promise, resolve, reject };
  };
  const WHERE = "src/rpc/client.rs:96";

  it("closes the editor only after the save succeeded and blocks a second save meanwhile", async () => {
    const save = deferred();
    const onEdit = vi.fn(() => save.promise);
    render(<PendingReviewBlock review={missed} prUrl={PR_URL} onEdit={onEdit} />);
    await userEvent.click(screen.getByRole("button", { name: `Edit comment on ${WHERE}` }));
    const field = screen.getByRole("textbox", { name: `Edit comment on ${WHERE}` });
    expect(field).toHaveValue(missed.comments[0]!.body);
    await userEvent.clear(field);
    const empty = screen.getByRole("button", { name: "Save" });
    expect(empty).toHaveAttribute("aria-disabled", "true");
    expect(empty).toHaveAccessibleDescription(/Write the comment text first\./);
    await userEvent.click(empty);
    expect(onEdit).not.toHaveBeenCalled();
    await userEvent.type(field, "Shorter.");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onEdit).toHaveBeenCalledWith("PRRC_fixture482_c1", "Shorter.");
    expect(screen.getByRole("textbox")).toHaveValue("Shorter.");
    expect(screen.getByRole("button", { name: "Saving…" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveAccessibleDescription(
      /Not available while the current request runs\./,
    );
    await userEvent.click(screen.getByRole("button", { name: "Saving…" }));
    expect(onEdit).toHaveBeenCalledOnce();
    save.resolve();
    await waitFor(() => expect(screen.queryByRole("textbox")).not.toBeInTheDocument());
  });

  it("keeps the typed text and shows the error when the save fails", async () => {
    const onEdit = vi.fn(() => Promise.reject(new Error("The pull request head changed.")));
    render(<PendingReviewBlock review={missed} prUrl={PR_URL} onEdit={onEdit} />);
    await userEvent.click(screen.getByRole("button", { name: `Edit comment on ${WHERE}` }));
    const field = screen.getByRole("textbox");
    await userEvent.clear(field);
    await userEvent.type(field, "My new text.");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Not saved: The pull request head changed.");
    expect(screen.getByRole("textbox")).toHaveValue("My new text.");
    expect(screen.getByRole("button", { name: "Save" })).not.toHaveAttribute("aria-disabled");
  });

  it("a delete answered comment_published shows the server's sentence, not 'Not deleted'", async () => {
    const sentence =
      "This comment was published on GitHub in the meantime; the published comment was deleted.";
    const onDelete = vi.fn(() =>
      Promise.reject(Object.assign(new Error(sentence), { code: "comment_published" })),
    );
    render(<PendingReviewBlock review={missed} prUrl={PR_URL} onDelete={onDelete} />);
    await userEvent.click(screen.getByRole("button", { name: `Delete comment on ${WHERE}` }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(sentence);
    expect(alert).not.toHaveTextContent("Not deleted");
  });

  it("deletes through the callback, blocks a second delete and shows a failure", async () => {
    const del = deferred();
    const onDelete = vi.fn(() => del.promise);
    render(<PendingReviewBlock review={missed} prUrl={PR_URL} onDelete={onDelete} />);
    const button = screen.getByRole("button", { name: `Delete comment on ${WHERE}` });
    await userEvent.click(button);
    expect(onDelete).toHaveBeenCalledWith("PRRC_fixture482_c1");
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription(/Not available while the current request runs\./);
    await userEvent.click(button);
    expect(onDelete).toHaveBeenCalledOnce();
    del.reject(new Error("GitHub is not reachable."));
    expect(await screen.findByRole("alert")).toHaveTextContent("Not deleted: GitHub is not reachable.");
    expect(button).not.toHaveAttribute("aria-disabled");
  });

  it("explains Edit, Delete, Save and Cancel in tooltips", async () => {
    render(<PendingReviewBlock review={missed} prUrl={PR_URL} onEdit={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.getByRole("button", { name: `Edit comment on ${WHERE}` })).toHaveAccessibleDescription(
      /pending review on GitHub/,
    );
    expect(screen.getByRole("button", { name: `Delete comment on ${WHERE}` })).toHaveAccessibleDescription(
      /Removes this comment from your pending review on GitHub/,
    );
    await userEvent.click(screen.getByRole("button", { name: `Edit comment on ${WHERE}` }));
    expect(screen.getByRole("button", { name: "Save" })).toHaveAccessibleDescription(/nothing is published/);
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveAccessibleDescription(/stays as it was/);
  });

  it("links each comment back to the step that shows its line, or says no step does", async () => {
    const open = vi.fn();
    const two = {
      ...missed,
      comments: [missed.comments[0]!, { ...missed.comments[0]!, id: "x", line: 999 }],
    };
    render(
      <PendingReviewBlock
        review={two}
        prUrl={PR_URL}
        placeOf={(c) => (c.id === "x" ? null : { label: "step 1.2", open: () => open(c.id) })}
      />,
    );
    const show = screen.getByRole("button", { name: "Show in step 1.2" });
    expect(show).toHaveAccessibleDescription(
      "Opens the step that shows this line, with the comment open there.",
    );
    await userEvent.click(show);
    expect(open).toHaveBeenCalledWith("PRRC_fixture482_c1");
    expect(screen.getByText("No step of this tour shows this line.")).toBeVisible();
  });

  it("is read-only without callbacks", () => {
    render(<PendingReviewBlock review={missed} prUrl={PR_URL} />);
    expect(screen.queryByRole("button", { name: /^(Edit|Delete) comment/ })).not.toBeInTheDocument();
  });

  it("says when the pending review has no comments", () => {
    render(
      <PendingReviewBlock
        review={{ commenting_available: true, comments: [], incomplete: false }}
        prUrl={PR_URL}
      />,
    );
    expect(screen.getByRole("article")).toHaveTextContent(
      "Your pending review on GitHub has no comments yet.",
    );
  });

  it("says when commenting is not available", () => {
    render(
      <PendingReviewBlock
        review={{ commenting_available: false, comments: [], incomplete: false }}
        prUrl={PR_URL}
      />,
    );
    expect(screen.getByRole("article")).toHaveTextContent(
      "Commenting is not available: the GitHub App is not installed on this repository.",
    );
    expect(screen.queryByRole("link", { name: /Finish on GitHub/ })).not.toBeInTheDocument();
  });

  it("never links a PR URL outside github.com (T12)", () => {
    render(<PendingReviewBlock review={missed} prUrl="javascript:alert(1)" />);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
  });
});

describe.each(FIXTURE_STATES)("quiet blocks in state %s", (state) => {
  const stateTour = seedFor(state, "tour") as Tour;
  const pending = seedFor(state, "pending-review") as PendingReview;

  it("render content and keep the wording rules", () => {
    const blocks = [
      <UnexplainedBlock key="u" items={stateTour.quiet.unexplained} />,
      <MovesBlock key="m" moves={stateTour.quiet.moves} />,
      ...stateTour.quiet.not_analysed.map((s) => (
        <NotAnalysedBlock key={s.category} section={s} counters={stateTour.pr_context.facts.counters} />
      )),
      <PendingReviewBlock key="p" review={pending} prUrl={stateTour.analysis.pr.html_url} />,
    ];
    for (const block of blocks) {
      const { container, unmount } = render(block);
      expect(screen.getByRole("heading", { level: 2 })).not.toBeEmptyDOMElement();
      expect(container.textContent!.length).toBeGreaterThan(40);
      expectTrustWording(container.textContent ?? "");
      unmount();
    }
  });
});

it("does not offer code for generated ranges rejected by the lines endpoint", () => {
  const generated = structuredClone(section(tour, "generated_mechanical"));
  generated.items = [
    {
      file: "api/generated.pb.go",
      side: "head",
      ranges: [{ start_line: 1, end_line: 5 }],
      rule: "generated file",
      badge: "classified_by_path",
    },
  ];
  render(
    <NotAnalysedBlock
      section={generated}
      counters={tour.pr_context.facts.counters}
      code={() => <div>code</div>}
    />,
  );
  expect(screen.queryByRole("button", { name: /Show lines/ })).toBeNull();
});

it("opens renderable mechanical lines, never generated or binary lines", async () => {
  const s = {
    category: "generated_mechanical" as const,
    file_count: 2,
    items: [
      {
        file: "Cargo.lock",
        side: "head" as const,
        ranges: [{ start_line: 1, end_line: 2 }],
        badge: "classified_by_path" as const,
        rule: "Cargo.lock",
        renderable: true,
      },
      {
        file: "generated.rs",
        side: "head" as const,
        ranges: [{ start_line: 3, end_line: 4 }],
        badge: "classified_by_path" as const,
        rule: "generated",
        renderable: false,
      },
    ],
  };
  render(
    <NotAnalysedBlock
      section={s}
      counters={tour.pr_context.facts.counters}
      code={(item) => <p>code of {item.file}</p>}
    />,
  );
  await userEvent.click(screen.getByRole("button", { name: "Show lines 1–2" }));
  expect(screen.getByText("code of Cargo.lock")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Show lines 3–4" })).toBeNull();
});
