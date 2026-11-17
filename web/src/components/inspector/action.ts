import { useRef, useState } from "react";

/** Plain message of a rejected callback; API errors carry user-facing text (http-api.md § Errors). */
export function messageOf(error: unknown): string {
  return error instanceof Error && error.message ? error.message : "Something went wrong. Try again.";
}

/** Grows a text field with its content, up to 170 px (mockup dock fields). */
export function growField(field: HTMLTextAreaElement | null): void {
  if (!field) return;
  field.style.height = "auto";
  field.style.height = `${Math.min(field.scrollHeight + 2, 170)}px`;
}

/** One callback at a time: `busy` while its promise is pending, `error` when it rejected. */
export function useAction() {
  const running = useRef(false);
  const [state, setState] = useState<{ busy: boolean; error: string | null }>({ busy: false, error: null });
  /** Runs `action` unless one is pending; resolves to whether it succeeded. */
  const run = async (action: () => Promise<unknown> | void): Promise<boolean> => {
    if (running.current) return false;
    running.current = true;
    setState({ busy: true, error: null });
    try {
      await action();
      setState({ busy: false, error: null });
      return true;
    } catch (error) {
      setState({ busy: false, error: messageOf(error) });
      return false;
    } finally {
      running.current = false;
    }
  };
  return { ...state, run, setError: (error: string | null) => setState((s) => ({ ...s, error })) };
}
