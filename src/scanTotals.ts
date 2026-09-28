// Keep scanner totals: hard-link deduplication means a parent need not equal
// the sum of its displayed children. Apply only changes since that scan.
export const adjustedScanTotal = (
  scannedTotal: number | undefined,
  children: Array<{ before: number; after: number }>
) => {
  const before = children.reduce((sum, child) => sum + child.before, 0);
  const after = children.reduce((sum, child) => sum + child.after, 0);
  return Math.max(0, (scannedTotal ?? before) + after - before);
};
