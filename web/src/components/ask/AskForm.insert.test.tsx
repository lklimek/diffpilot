import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AskForm, lineReference } from "./AskForm";

const step5 = { id: "f1.5", label: "Step 5" };
const sections = (open: boolean) => ({ isOpen: () => open, toggle: vi.fn(), reveal: vi.fn() });

describe("lineReference", () => {
  it("names a line or a range as path:line, old code said", () => {
    const path = "packages/foo/src/bar.rs";
    expect(lineReference({ path, side: "head", start_line: null, line: 123 })).toBe(`${path}:123`);
    expect(lineReference({ path, side: "head", start_line: 120, line: 123 })).toBe(`${path}:120-123`);
    expect(lineReference({ path, side: "base", start_line: null, line: 7 })).toBe(`${path}:7 (old code)`);
  });
});

describe("AskForm: text inserted from a code line", () => {
  it("goes in at the cursor with spaces around it, focuses the field and sends nothing", async () => {
    const onAsk = vi.fn();
    const onInserted = vi.fn();
    const { rerender } = render(<AskForm step={step5} onAsk={onAsk} sections={sections(true)} />);
    const field = screen.getByRole<HTMLTextAreaElement>("textbox");
    await userEvent.type(field, "Why does do this?");
    field.setSelectionRange(9, 9);
    field.blur();
    rerender(
      <AskForm
        step={step5}
        onAsk={onAsk}
        sections={sections(true)}
        insert={{ text: "src/a.rs:12" }}
        onInserted={onInserted}
      />,
    );
    expect(field).toHaveValue("Why does src/a.rs:12 do this?");
    expect(field).toHaveFocus();
    expect(field.selectionStart).toBe("Why does src/a.rs:12 ".length);
    expect(onInserted).toHaveBeenCalledOnce();
    expect(onAsk).not.toHaveBeenCalled();
  });

  it("opens a collapsed Ask first", () => {
    const closed = sections(false);
    render(
      <AskForm
        step={step5}
        onAsk={() => {}}
        sections={closed}
        insert={{ text: "a.rs:1" }}
        onInserted={() => {}}
      />,
    );
    expect(closed.reveal).toHaveBeenCalledWith("ask");
  });
});
