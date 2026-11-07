export type FileSelection = {
  ids: Set<string>;
  anchorId: string | null;
};

export const emptyFileSelection = (): FileSelection => ({
  ids: new Set(),
  anchorId: null,
});

// Use every expanded row in display order, including virtualized offscreen rows.
export const selectFile = (
  selection: FileSelection,
  orderedIds: readonly string[],
  targetId: string,
  { range = false, toggle = false } = {}
): FileSelection => {
  const targetIndex = orderedIds.indexOf(targetId);
  if (targetIndex < 0) return selection;

  const anchorIndex = selection.anchorId
    ? orderedIds.indexOf(selection.anchorId)
    : -1;
  const hasRange = range && anchorIndex >= 0;
  const ids = hasRange || toggle ? new Set(selection.ids) : new Set<string>();
  const selecting = !toggle || !selection.ids.has(targetId);
  const start = hasRange ? Math.min(anchorIndex, targetIndex) : targetIndex;
  const end = hasRange ? Math.max(anchorIndex, targetIndex) : targetIndex;
  for (let index = start; index <= end; index++) {
    if (selecting) ids.add(orderedIds[index]);
    else ids.delete(orderedIds[index]);
  }
  return { ids, anchorId: hasRange ? selection.anchorId : targetId };
};

// A queued directory covers its descendants. Check ancestors by path segment,
// rather than repeatedly comparing every pair in a potentially large batch.
export const mergeDeleteCandidates = <T extends { id: string }>(
  current: readonly T[],
  additions: readonly T[]
): T[] => {
  const byId = new Map([...current, ...additions].map((item) => [item.id, item]));
  return [...byId.values()].filter((item) => {
    let slash = item.id.lastIndexOf("/");
    while (slash > 0) {
      if (byId.has(item.id.slice(0, slash))) return false;
      slash = item.id.lastIndexOf("/", slash - 1);
    }
    return true;
  });
};
