import { describe, it, expect, beforeEach } from "vitest";
import { __setForTest, getInput, getOutput, subscribe } from "./levels";

describe("levels store", () => {
  beforeEach(() => __setForTest(0, 0));

  it("starts at zero", () => {
    expect(getInput()).toBe(0);
    expect(getOutput()).toBe(0);
  });

  it("updates snapshots and notifies subscribers", () => {
    let notified = 0;
    const unsub = subscribe(() => notified++);
    __setForTest(0.5, 0.25);
    expect(getInput()).toBe(0.5);
    expect(getOutput()).toBe(0.25);
    expect(notified).toBe(1);
    unsub();
    __setForTest(0.1, 0.1);
    expect(notified).toBe(1); // no longer subscribed
  });
});
