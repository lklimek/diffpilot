import { setupWorker } from "msw/browser";
import { FIXTURE_STATES, setFixtureState, type FixtureState } from "./fixtures";
import { handlers } from "./handlers";
import { SCENARIO_NAMES, setScenario, type ScenarioName } from "./scenarios";

const isFixtureState = (value: string): value is FixtureState =>
  (FIXTURE_STATES as readonly string[]).includes(value);

const isScenario = (value: string): value is ScenarioName =>
  (SCENARIO_NAMES as readonly string[]).includes(value);

/**
 * Starts msw in the browser; `?state=<wrong|missed|noq|noprofile|resume>` selects the fixture state,
 * `?scenario=<name>` a screen scenario (scenarios.ts).
 */
export async function startMockApi(): Promise<void> {
  const search = new URLSearchParams(window.location.search);
  const requested = search.get("state");
  if (requested !== null) {
    if (!isFixtureState(requested)) throw new Error(`unknown fixture state "${requested}"`);
    setFixtureState(requested);
  }
  const named = search.get("scenario");
  if (named !== null) {
    if (!isScenario(named)) throw new Error(`unknown scenario "${named}"`);
    setScenario(named);
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
