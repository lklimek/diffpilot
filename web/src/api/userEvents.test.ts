import { userEventNotice } from "../app/userEvents";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../__tests__/fake-event-source";
import { createApiClient } from "./client";
import type { UserEvent, UserStreamEvent } from "./generated/types";

const EVENT: UserEvent = {
  id: "0f7c1d1e-0000-4000-8000-000000000001",
  kind: "questions_ready",
  pr: { repo: { owner: "acme", name: "node-client" }, number: 482 },
  count: 3,
  text: "acme/node-client #482: 3 questions ready",
  url: "/pr/acme/node-client/482/comments",
  at: "2026-11-22T09:30:00Z",
};

const emit = (source: FakeEventSource, event: UserStreamEvent | { event: string; data: string }) =>
  source.emit("data" in event ? event : { event: event.event, data: JSON.stringify(event) });

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => vi.unstubAllGlobals());

describe("subscribeUserEvents (RW3, sse-events.md)", () => {
  it("delivers the stored event of each `user_event` and skips heartbeats and mismatched data", () => {
    const seen: UserEvent[] = [];
    const close = createApiClient().subscribeUserEvents({ onEvent: (e) => seen.push(e), onError: () => {} });
    const source = FakeEventSource.latest();
    expect(source.url).toBe("/api/v1/events");
    emit(source, { event: "heartbeat", at: "2026-11-22T09:30:15Z" });
    emit(source, { event: "user_event", data: JSON.stringify({ event: "heartbeat", at: "x" }) });
    emit(source, { event: "user_event", data: "{not json" });
    emit(source, { event: "user_event", user_event: EVENT });
    expect(seen).toEqual([EVENT]);
    close();
    expect(source.readyState).toBe(FakeEventSource.CLOSED);
  });
});

it.each(["questions_ready", "round_executed"] as const)("opens Comments from a %s notification", (kind) => {
  expect(userEventNotice({ ...EVENT, kind }).path).toBe("/pr/acme/node-client/482/comments");
});
