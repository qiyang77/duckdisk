import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(new URL("../src/fileSelection.ts", import.meta.url), "utf8");
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ESNext },
});
const { emptyFileSelection, selectFile, mergeDeleteCandidates } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString("base64")}`
);
const order = ["/a", "/b", "/c", "/d", "/e"];
const selected = (selection) => [...selection.ids].sort();
const candidates = (...ids) => ids.map((id) => ({ id }));

test("checkboxes toggle independent items; an ordinary row click replaces the selection", () => {
  let state = selectFile(emptyFileSelection(), order, "/a", { toggle: true });
  state = selectFile(state, order, "/d", { toggle: true });
  assert.deepEqual(selected(state), ["/a", "/d"]);
  state = selectFile(state, order, "/a", { toggle: true });
  assert.deepEqual(selected(state), ["/d"]);
  state = selectFile(state, order, "/c");
  assert.deepEqual(selected(state), ["/c"]);
});

test("Shift selects a range in either direction and retains its original anchor", () => {
  let state = selectFile(emptyFileSelection(), order, "/d");
  state = selectFile(state, order, "/b", { range: true });
  assert.deepEqual(selected(state), ["/b", "/c", "/d"]);
  assert.equal(state.anchorId, "/d");
  state = selectFile(state, order, "/e", { range: true });
  assert.deepEqual(selected(state), ["/b", "/c", "/d", "/e"]);
});

test("Shift uses the current sorted order and spans rows outside the virtual viewport", () => {
  const ids = Array.from({ length: 1000 }, (_, index) => `/item-${index}`).reverse();
  let state = selectFile(emptyFileSelection(), ids, ids[10]);
  state = selectFile(state, ids, ids[900], { range: true });
  assert.equal(state.ids.size, 891);
  assert.deepEqual([...state.ids], ids.slice(10, 901));
});

test("Shift-checkbox can uncheck a range without losing unrelated selected items", () => {
  let state = { ids: new Set(order), anchorId: "/b" };
  state = selectFile(state, order, "/d", { range: true, toggle: true });
  assert.deepEqual(selected(state), ["/a", "/e"]);
});

test("a removed anchor falls back to the clicked item; ineligible targets are ignored", () => {
  const state = { ids: new Set(["/a"]), anchorId: "/removed" };
  assert.deepEqual(selected(selectFile(state, order, "/d", { range: true })), ["/d"]);
  assert.equal(selectFile(state, order, "/not-selectable", { range: true }), state);
});

test("batch staging collapses parents and descendants in any input order", () => {
  for (const additions of [
    candidates("/folder/a", "/folder", "/folder/sub/b"),
    candidates("/folder", "/folder/sub/b", "/folder/a"),
  ]) {
    assert.deepEqual(mergeDeleteCandidates([], additions), candidates("/folder"));
  }
  assert.deepEqual(
    mergeDeleteCandidates(candidates("/folder/a", "/other"), candidates("/folder")),
    candidates("/other", "/folder")
  );
});

test("staging respects path boundaries and keeps similarly named siblings", () => {
  assert.deepEqual(
    mergeDeleteCandidates(candidates("/foo"), candidates("/foo-bar", "/foobar", "/foo/child")),
    candidates("/foo", "/foo-bar", "/foobar")
  );
});

test("duplicate candidates update their metadata without duplicating entries", () => {
  const original = { id: "/a", size: 1 };
  const updated = { id: "/a", size: 2 };
  assert.deepEqual(mergeDeleteCandidates([original], [updated]), [updated]);
});

test("large batches retain independent files and remove covered descendants", () => {
  const items = Array.from({ length: 10000 }, (_, index) => ({ id: `/folder/file-${index}` }));
  assert.equal(mergeDeleteCandidates([], items).length, 10000);
  assert.deepEqual(mergeDeleteCandidates(items, candidates("/folder")), candidates("/folder"));
});
