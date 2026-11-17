import { describe, expect, it, vi } from "vitest";
import type { ReviewState } from "./generated/types";
import { createReviewStateStore } from "./review-state";

const ID = "a1";

/** A promise with its resolve / reject handles. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const state = (ticked: string[], step: string | null = null): ReviewState => ({
  ticked_steps: ticked,
  last_viewed: step ? { block: "step", step_id: step } : null,
});
const flush = () => new Promise((r) => setTimeout(r, 0));

/** A store over a fake client whose GET and PUT answers the test controls. */
function setup() {
  const gets: ReturnType<typeof deferred<ReviewState>>[] = [];
  const puts: { body: ReviewState; reply: ReturnType<typeof deferred<void>> }[] = [];
  const store = createReviewStateStore({
    getReviewState: vi.fn(() => {
      const d = deferred<ReviewState>();
      gets.push(d);
      return d.promise;
    }),
    putReviewState: vi.fn((_id: string, body: ReviewState) => {
      const reply = deferred<void>();
      puts.push({ body, reply });
      return reply.promise;
    }),
  });
  const tick = (step: string) => store.change(ID, (s) => ({ ...s, ticked_steps: [...s.ticked_steps, step] }));
  return { store, gets, puts, tick };
}

describe("review-state store", () => {
  it("is loading until the first read, then shows the stored state", async () => {
    const { store, gets } = setup();
    const reading = store.read(ID);
    expect(store.view(ID).status).toBe("loading");
    gets[0]!.resolve(state(["f2.1"], "f1.5"));
    await reading;
    expect(store.view(ID)).toMatchObject({ status: "ready", state: state(["f2.1"], "f1.5") });
  });

  it("a read that finishes after a local tick does not take the tick back", async () => {
    const { store, gets, puts, tick } = setup();
    const first = store.read(ID);
    gets[0]!.resolve(state(["f2.1"]));
    await first;
    const late = store.read(ID);
    tick("f1.1");
    store.change(ID, (s) => ({ ...s, last_viewed: { block: "step", step_id: "f1.1" } }));
    gets[1]!.resolve(state(["f2.1"]));
    await late;
    expect(store.view(ID).state).toEqual(state(["f2.1", "f1.1"], "f1.1"));
    puts[0]!.reply.resolve();
    await flush();
    expect(puts.at(-1)!.body).toEqual(state(["f2.1", "f1.1"], "f1.1"));
    puts.at(-1)!.reply.resolve();
    await flush();
    expect(store.view(ID).state).toEqual(state(["f2.1", "f1.1"], "f1.1"));
  });

  it("a read that started before a save completed is ignored: the saved tick stays", async () => {
    const { store, gets, puts, tick } = setup();
    const first = store.read(ID);
    gets[0]!.resolve(state([]));
    await first;
    tick("f1.1");
    // The screen is opened again while the PUT is in flight: its GET still sees the old state.
    const reopened = store.read(ID);
    puts[0]!.reply.resolve();
    await flush();
    gets[1]!.resolve(state([]));
    await reopened;
    expect(store.view(ID).state.ticked_steps).toEqual(["f1.1"]);
    tick("f1.2");
    expect(puts.at(-1)!.body.ticked_steps).toEqual(["f1.1", "f1.2"]);
  });

  it("never writes before a read succeeded; then merges the changes made meanwhile and saves them", async () => {
    const { store, gets, puts, tick } = setup();
    const failed = store.read(ID);
    gets[0]!.reject(new Error("down"));
    await failed;
    expect(store.view(ID)).toMatchObject({ status: "unreadable", loadError: new Error("down") });
    // A change retries the read; until it succeeds nothing is written.
    tick("f1.1");
    expect(gets).toHaveLength(2);
    store.change(ID, (s) => ({ ...s, last_viewed: { block: "step", step_id: "f1.1" } }));
    expect(store.view(ID).state).toEqual(state(["f1.1"], "f1.1"));
    expect(puts).toEqual([]);
    gets[1]!.resolve(state(["f2.1"], "f1.5"));
    await flush();
    // Stored ticks plus the ticks made meanwhile; the block the user is on wins.
    expect(store.view(ID)).toMatchObject({ status: "ready", state: state(["f2.1", "f1.1"], "f1.1") });
    expect(puts.map((p) => p.body)).toEqual([state(["f2.1", "f1.1"], "f1.1")]);
  });

  it("a successful first read with no changes meanwhile writes nothing", async () => {
    const { store, gets, puts } = setup();
    const failed = store.read(ID);
    gets[0]!.reject(new Error("down"));
    await failed;
    const retry = store.read(ID);
    gets[1]!.resolve(state(["f2.1"], "f1.5"));
    await retry;
    expect(store.view(ID)).toMatchObject({ status: "ready", state: state(["f2.1"], "f1.5") });
    expect(puts).toEqual([]);
  });

  it("keeps one queue per analysis: PUTs in order, only the newest waiting state is sent", async () => {
    const { store, gets, puts, tick } = setup();
    const read = store.read(ID);
    gets[0]!.resolve(state([]));
    await read;
    tick("f1.1");
    // Screen left and opened again while the first PUT is slow: changes join the same queue.
    tick("f1.2");
    tick("f1.3");
    expect(puts.map((p) => p.body.ticked_steps)).toEqual([["f1.1"]]);
    puts[0]!.reply.resolve();
    await flush();
    expect(puts.map((p) => p.body.ticked_steps)).toEqual([["f1.1"], ["f1.1", "f1.2", "f1.3"]]);
    puts[1]!.reply.resolve();
    await flush();
    expect(store.view(ID)).toMatchObject({ state: state(["f1.1", "f1.2", "f1.3"]), saveError: null });
  });

  it("reports a failed save and keeps the local state until a later save succeeds", async () => {
    const { store, gets, puts, tick } = setup();
    const read = store.read(ID);
    gets[0]!.resolve(state([]));
    await read;
    tick("f1.1");
    puts[0]!.reply.reject(new Error("offline"));
    await flush();
    expect(store.view(ID)).toMatchObject({ state: state(["f1.1"]), saveError: new Error("offline") });
    tick("f1.2");
    puts[1]!.reply.resolve();
    await flush();
    expect(store.view(ID)).toMatchObject({ state: state(["f1.1", "f1.2"]), saveError: null });
  });
});
