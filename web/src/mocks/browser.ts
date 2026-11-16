import { setupWorker } from "msw/browser";
import { FIXTURE_STATES, setFixtureState, type FixtureState } from "./fixtures";
import { handlers } from "./handlers";

const isFixtureState = (value: string): value is FixtureState =>
  (FIXTURE_STATES as readonly string[]).includes(value);

/** Starts msw in the browser; `?state=<wrong|missed|noq|noprofile|resume>` selects the fixture state. */
export async function startMockApi(): Promise<void> {
  const requested = new URLSearchParams(window.location.search).get("state");
  if (requested !== null) {
    if (!isFixtureState(requested)) throw new Error(`unknown fixture state "${requested}"`);
    setFixtureState(requested);
  }
  await setupWorker(...handlers).start({
    quiet: true,
    onUnhandledFrame({ frame, defaults }) {
      const request = (frame.data as { request?: Request }).request;
      const pathname = request ? new URL(request.url).pathname : "";
      if (pathname.startsWith("/api/") || pathname.startsWith("/auth/")) defaults.error();
    },
  });
}
