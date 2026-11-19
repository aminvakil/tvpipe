import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { test } from "node:test";

import { play, preview, testPattern, TvpipeError, version, type TvpipeEvent } from "../src/index.ts";
import { opts, realBinary, sleep, until, Viewer, within } from "./helpers.ts";

test("version", async (t) => {
  assert.match(await version({ binary: realBinary() }), /^\d+\.\d+\.\d+/);
});

test("test pattern to a browser tab: events, a viewer, stop", async (t) => {
  const seen: string[] = [];
  const s = await within(testPattern({ ...opts(t), to: "browser", size: "320x240" }), 30000, "testPattern to start");
  t.after(() => s.kill());
  s.on("event", (ev) => seen.push(ev.type));
  assert.match(s.playerUrl ?? "", /^http:\/\/127\.0\.0\.1:\d+\/\?t=[0-9a-f]+$/);
  assert.equal(s.info?.route, "builtin");
  assert.deepEqual([s.info?.video.width, s.info?.video.height], [320, 240]);
  assert.equal(s.receivers.get("browser")?.state, "connected");
  assert.equal(s.receivers.get("browser")?.protocol, "browser");
  const viewers: string[] = [];
  s.on("viewer", (v) => viewers.push(v.state));

  const viewer = new Viewer(s.playerUrl!);
  await viewer.opened;
  await until("video at the viewer", () => viewer.count(3) >= 10);
  const config = viewer.config() as { video: { width: number } } | undefined;
  assert.equal(config?.video.width, 320);
  assert.ok(viewer.count(2) > 0, "audio at the viewer");
  await until("stats", () => s.stats !== undefined);
  assert.ok(viewers.includes("connected"));

  const end = await within(s.stop(), 20000, "stop");
  assert.equal(end.reason, "stopped");
  assert.equal(end.exitCode, 0);
  assert.equal(end.receivers[0]?.name, "browser");
  assert.ok((end.receivers[0]?.pushed ?? 0) > 0);
  assert.equal(s.running, false);
  assert.equal(s.receivers.get("browser")?.state, "stopped");
  assert.deepEqual(await s.done, end);
  // Stopping again is harmless.
  assert.deepEqual(await s.stop(), end);
  viewer.close();
  assert.ok(seen.includes("stats"));
});

test("preview without input is the test pattern; async dispose stops it", async (t) => {
  const s = await within(preview(undefined, opts(t)), 30000, "preview to start");
  t.after(() => s.kill());
  assert.ok(s.playerUrl);
  const events: TvpipeEvent[] = [];
  const collecting = (async () => {
    for await (const ev of s.events()) events.push(ev);
  })();
  await sleep(300);
  await within(s[Symbol.asyncDispose](), 20000, "async dispose");
  await within(collecting, 5000, "the event iterator to end");
  assert.equal(s.running, false);
  assert.ok(events.some((e) => e.type === "end"));
});

test("an abort signal stops the session", async (t) => {
  const ac = new AbortController();
  const s = await within(testPattern({ ...opts(t), to: "browser", signal: ac.signal }), 30000, "testPattern to start");
  t.after(() => s.kill());
  ac.abort();
  const end = await within(s.done, 20000, "the end after abort");
  assert.equal(end.reason, "stopped");
  // Already aborted: no process at all.
  await assert.rejects(
    testPattern({ ...opts(t), to: "browser", signal: ac.signal }),
    (e: unknown) => e instanceof TvpipeError && e.code === "aborted",
  );
});

test("aborted while waiting for a viewer", async (t) => {
  const ac = new AbortController();
  setTimeout(() => ac.abort(), 500);
  await assert.rejects(
    testPattern({ ...opts(t), to: "browser", player: { waitForViewer: true }, signal: ac.signal }),
    (e: unknown) => e instanceof TvpipeError && e.code === "aborted",
  );
});

test("errors carry a message and a stable code", async (t) => {
  await assert.rejects(play("/no/such/file.mp4", { ...opts(t), to: "browser" }), (e: unknown) => {
    assert.ok(e instanceof TvpipeError);
    assert.equal(e.code, "input-not-found");
    assert.equal(e.kind, "not-found");
    assert.match(e.message, /no such file/);
    assert.equal(e.exitCode, 1);
    return true;
  });
  // Usage errors from the argument parser (stderr, exit status 2).
  await assert.rejects(
    play("x.mp4", { ...opts(t), to: "browser", latency: Number.NaN }),
    (e: unknown) => e instanceof TvpipeError && e.code === "usage" && /latency/i.test(e.message),
  );
  await assert.rejects(
    testPattern({ ...opts(t), to: "tv", player: { port: 1 } }),
    (e: unknown) => e instanceof TvpipeError && e.code === "invalid-argument",
  );
});

test("tvpipe stops when the program that started it dies", async (t) => {
  // A child Node process starts a session, prints tvpipe's pid and is
  // killed; tvpipe sees its stdin close and exits by itself.
  const script = `
    import { testPattern } from ${JSON.stringify(new URL("../src/index.ts", import.meta.url).href)};
    const s = await testPattern({ binary: ${JSON.stringify(realBinary())}, configDir: ${JSON.stringify(opts(t).configDir)}, to: "browser" });
    console.log(s.pid);
    setInterval(() => {}, 1000);
  `;
  const child = spawn(process.execPath, ["--input-type=module", "-e", script], {
    stdio: ["ignore", "pipe", "inherit"],
  });
  const pid = await new Promise<number>((resolve, reject) => {
    child.stdout.once("data", (d: Buffer) => resolve(Number(d.toString().trim())));
    child.once("exit", () => reject(new Error("the child exited early")));
  });
  assert.ok(pid > 0);
  t.after(() => {
    try {
      process.kill(pid, "SIGKILL");
    } catch {
      // Gone, as it should be.
    }
  });
  child.kill("SIGKILL");
  const alive = () => {
    try {
      process.kill(pid, 0);
      return true;
    } catch {
      return false;
    }
  };
  await until("tvpipe to exit", () => !alive(), 10000);
});
