import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";

const source = readFileSync(new URL("../src/scanTotals.ts", import.meta.url), "utf8");
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext },
});
const { adjustedScanTotal } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString("base64")}`
);

assert.equal(adjustedScanTotal(80, [{ before: 100, after: 100 }]), 80);
assert.equal(adjustedScanTotal(80, [{ before: 100, after: 70 }]), 50);
assert.equal(adjustedScanTotal(80, [{ before: 100, after: 110 }]), 90);
assert.equal(adjustedScanTotal(20, [{ before: 100, after: 0 }]), 0);
assert.equal(adjustedScanTotal(undefined, [{ before: 100, after: 70 }]), 70);
assert.equal(adjustedScanTotal(80, []), 80);
assert.equal(adjustedScanTotal(100, [{ before: 20, after: 0 }]), 80);
assert.equal(adjustedScanTotal(100, [{ before: 20, after: 30 }]), 110);
console.log("Scan total reconciliation: 8 assertions passed");
