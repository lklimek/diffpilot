import { useRef, useState, type Ref } from "react";

/** Plain message of a rejected callback; API errors carry user-facing text (http-api.md § Errors). */
export function messageOf(error: unknown): string {
  return error instanceof Error && error.message ? error.message : "Something went wrong. Try again.";
}

/** The API error code of a rejected callback (`ApiError.code`), if it has one. */
export function codeOf(error: unknown): string | null {
  const code = (error as { code?: unknown } | null)?.code;
  return typeof code === "string" ? code : null;
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
  const [state, setState] = useState<{ busy: boolean; error: string | null; code: string | null }>({
    busy: false,
    error: null,
    code: null,
  });
  /** Runs `action` unless one is pending; resolves to whether it succeeded. */
  const run = async (action: () => Promise<unknown> | void): Promise<boolean> => {
    if (running.current) return false;
    running.current = true;
    setState({ busy: true, error: null, code: null });
    try {
      await action();
      setState({ busy: false, error: null, code: null });
      return true;
    } catch (error) {
      setState({ busy: false, error: messageOf(error), code: codeOf(error) });
      return false;
    } finally {
      running.current = false;
    }
  };
  return {
    ...state,
    run,
    setError: (error: string | null) => setState((s) => ({ ...s, error, code: null })),
  };
}

/** Sets a ref passed in by the caller (callback or object ref). */
export function assignRef<T>(ref: Ref<T> | undefined, value: T | null): void {
  if (typeof ref === "function") ref(value);
  else if (ref) ref.current = value;
}
