// A controllable stand-in for the browser EventSource (jsdom has none); install with vi.stubGlobal.
import type { AnalysisEvent } from "../api/generated/types";

type Listener = (event: MessageEvent<string>) => void;

/** Fake EventSource: tests open, emit, drop (browser retries) or fail (no retry) the stream. */
export class FakeEventSource {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 2;
  /** Every instance created since the last `reset`. */
  static instances: FakeEventSource[] = [];

  readyState = FakeEventSource.CONNECTING;
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  private listeners = new Map<string, Set<Listener>>();

  constructor(readonly url: string) {
    FakeEventSource.instances.push(this);
  }

  /** Forgets all instances. */
  static reset(): void {
    FakeEventSource.instances = [];
  }

  /** The most recently created instance. */
  static latest(): FakeEventSource {
    const latest = FakeEventSource.instances.at(-1);
    if (!latest) throw new Error("no EventSource was created");
    return latest;
  }

  addEventListener(type: string, listener: Listener): void {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(listener);
  }

  removeEventListener(type: string, listener: Listener): void {
    this.listeners.get(type)?.delete(listener);
  }

  close(): void {
    this.readyState = FakeEventSource.CLOSED;
  }

  /** The connection opened. */
  open(): void {
    this.readyState = FakeEventSource.OPEN;
    this.onopen?.();
  }

  /** The server sent `event` (named SSE event, JSON data). */
  emit(event: AnalysisEvent | { event: string; data: string }): void {
    const data = "data" in event && typeof event.data === "string" ? event.data : JSON.stringify(event);
    for (const listener of this.listeners.get(event.event) ?? []) {
      listener(new MessageEvent(event.event, { data }));
    }
  }

  /** The connection dropped; the browser retries on its own. */
  drop(): void {
    this.readyState = FakeEventSource.CONNECTING;
    this.onerror?.();
  }

  /** The connection failed for good (HTTP error); the browser does not retry. */
  fail(): void {
    this.readyState = FakeEventSource.CLOSED;
    this.onerror?.();
  }
}
