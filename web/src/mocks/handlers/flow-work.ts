import { HttpResponse } from "msw";
import { forAnalysis, route } from "./common";
import { liveAnalysis, setBackground } from "../live-analysis";
import { continueLazyFlows, requestLazyFlow } from "../lazy-flows";
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
    setBackground(body.enabled);
    continueLazyFlows();
    return HttpResponse.json(liveAnalysis());
  }),
);
