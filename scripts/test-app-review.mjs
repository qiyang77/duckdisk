import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { setImmediate } from "node:timers/promises";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

// Exercise the production eligibility logic and React effect callback without
// loading the disk UI or adding a DOM/test-renderer dependency.
const source = readFileSync(new URL("../src/components/DiskDetail.tsx", import.meta.url), "utf8");
const sourceFile = ts.createSourceFile("DiskDetail.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const declarations = sourceFile.statements.filter((statement) =>
  ts.isVariableStatement(statement) && statement.declarationList.declarations.some(
    ({ name }) => ts.isIdentifier(name) && (
      name.text.startsWith("review") || name.text === "recordSuccessfulScanForReview"
    )
  )
);
let reviewEffect;
function findEffect(node) {
  if (
    ts.isCallExpression(node) && node.expression.getText(sourceFile) === "useEffect" &&
    node.arguments[0]?.getText(sourceFile).includes("void recordSuccessfulScanForReview()")
  ) {
    reviewEffect = node.arguments[0];
  }
  ts.forEachChild(node, findEffect);
}
findEffect(sourceFile);
assert.ok(reviewEffect, "The scan completion review effect must be tested");
const { outputText } = ts.transpileModule(
  `${declarations.map((node) => node.getText(sourceFile)).join("\n")}\n` +
  `globalThis.runReviewEffect = ${reviewEffect.getText(sourceFile)};\n` +
  "globalThis.recordScan = recordSuccessfulScanForReview;",
  { compilerOptions: { target: ts.ScriptTarget.ESNext, module: ts.ModuleKind.ESNext } }
);

const day = 24 * 60 * 60 * 1000;
const now = Date.UTC(2026, 10, 8);
const firstKey = "duckdisk-review-first-successful-scan-at";
const countKey = "duckdisk-review-successful-scan-count";
const lastKey = "duckdisk-review-last-request-at";

function harness({ values = {}, mas = true, requestError } = {}) {
  const storage = new Map(Object.entries(values).map(([key, value]) => [key, String(value)]));
  const timers = new Map();
  const requests = [];
  const warnings = [];
  let timerId = 0;
  const context = vm.createContext({
    isMacAppStore: mas,
    Date: { now: () => now },
    localStorage: {
      getItem: (key) => storage.get(key) ?? null,
      setItem: (key, value) => storage.set(key, value),
    },
    invoke: async (command) => {
      requests.push(command);
      if (requestError) throw requestError;
    },
    console: { warn: (...args) => warnings.push(args) },
    window: {
      setTimeout: (callback, delay) => {
        assert.equal(delay, 1500);
        timers.set(++timerId, callback);
        return timerId;
      },
      clearTimeout: (id) => timers.delete(id),
    },
    view: "disk",
    rootNode: { id: "/test" },
    reviewRecordedForScan: { current: false },
  });
  vm.runInContext(outputText, context);
  return {
    context, storage, timers, requests, warnings,
    recordScan: () => context.recordScan(),
    effect: () => context.runReviewEffect(),
    async fireTimers() {
      for (const [id, callback] of [...timers]) {
        timers.delete(id);
        callback();
      }
      await setImmediate();
    },
  };
}

const eligible = { [firstKey]: now - day, [countKey]: 1 };

test("direct-download builds never record scans or request an App Store review", async () => {
  const h = harness({ mas: false });
  assert.equal(h.effect(), undefined);
  await h.recordScan();
  assert.equal(h.storage.size, 0);
  assert.equal(h.timers.size, 0);
  assert.deepEqual(h.requests, []);
});

test("first successful scan starts the age and usage counters without prompting", async () => {
  const h = harness();
  await h.recordScan();
  assert.equal(h.storage.get(firstKey), String(now));
  assert.equal(h.storage.get(countKey), "1");
  assert.deepEqual(h.requests, []);
});

test("both two successful scans and one day of use are required", async () => {
  for (const values of [
    { [firstKey]: now - 2 * day, [countKey]: 0 },
    { [firstKey]: now - day + 1, [countKey]: 1 },
  ]) {
    const h = harness({ values });
    await h.recordScan();
    assert.deepEqual(h.requests, []);
  }
});

test("an eligible completed scan requests the native review once and starts cooldown", async () => {
  const h = harness({ values: eligible });
  h.effect();
  assert.deepEqual(h.requests, []);
  await h.fireTimers();
  assert.deepEqual(h.requests, ["request_app_review"]);
  assert.equal(h.storage.get(countKey), "2");
  assert.equal(h.storage.get(lastKey), String(now));
  assert.equal(h.effect(), undefined);
});

test("cooldown suppresses requests until exactly 30 days have elapsed", async () => {
  for (const [elapsed, expected] of [[30 * day - 1, 0], [30 * day, 1]]) {
    const h = harness({ values: { ...eligible, [lastKey]: now - elapsed } });
    await h.recordScan();
    assert.equal(h.requests.length, expected);
  }
});

test("a failed native invocation does not consume the 30-day cooldown", async () => {
  const h = harness({ values: eligible, requestError: new Error("IPC unavailable") });
  await assert.rejects(h.recordScan(), /IPC unavailable/);
  assert.equal(h.storage.has(lastKey), false);
});

test("loading and missing scan results cannot schedule a review", () => {
  const h = harness({ values: eligible });
  h.context.view = "loading";
  assert.equal(h.effect(), undefined);
  h.context.view = "disk";
  h.context.rootNode = null;
  assert.equal(h.effect(), undefined);
  assert.equal(h.timers.size, 0);
});

test("a folder refresh during the delay reschedules instead of losing the review", async () => {
  const h = harness({ values: eligible });
  const cleanup = h.effect();
  cleanup(); // React cleans up the effect when rootNode changes.
  h.context.rootNode = { id: "/test", refreshed: true };
  h.effect();
  await h.fireTimers();
  assert.deepEqual(h.requests, ["request_app_review"]);
  assert.equal(h.storage.get(countKey), "2");
});

test("effect setup-cleanup-setup leaves one live request", async () => {
  const h = harness({ values: eligible });
  const cleanup = h.effect();
  cleanup();
  h.effect();
  assert.equal(h.timers.size, 1);
  await h.fireTimers();
  assert.equal(h.requests.length, 1);
});

test("leaving the result before the delay cancels the pending request", async () => {
  const h = harness({ values: eligible });
  const cleanup = h.effect();
  cleanup();
  h.context.view = "loading";
  h.effect();
  await h.fireTimers();
  assert.deepEqual(h.requests, []);
  assert.equal(h.storage.get(countKey), "1");
});

test("existing users retain their history and use the shorter cooldown after updating", async () => {
  const h = harness({ values: {
    [firstKey]: now - 100 * day,
    [countKey]: 12,
    [lastKey]: now - 31 * day,
  } });
  await h.recordScan();
  assert.deepEqual(h.requests, ["request_app_review"]);
  assert.equal(h.storage.get(firstKey), String(now - 100 * day));
  assert.equal(h.storage.get(countKey), "13");
});
