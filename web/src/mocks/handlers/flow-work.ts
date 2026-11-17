import { HttpResponse } from "msw";
import { forAnalysis, route } from "./common";
import { liveAnalysis, setBackground } from "../live-analysis";
import { requestLazyFlow } from "../lazy-flows";
export const requestFlow = route(
  "requestFlow",
  forAnalysis(({ params }) => {
    requestLazyFlow(String(params.flow));
    return HttpResponse.json(liveAnalysis());
  }),
);
export const setBackgroundAnalysis = route(
  "setBackgroundAnalysis",
  forAnalysis(async ({ request }) => {
    const body = (await request.json()) as { enabled: boolean };
    return HttpResponse.json(setBackground(body.enabled));
  }),
);
