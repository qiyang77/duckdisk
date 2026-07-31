export const TREE_ROW_HEIGHT = 33;
export const TREE_ROW_OVERSCAN = 20;

export type VirtualRange = {
  start: number;
  end: number;
  paddingTop: number;
  paddingBottom: number;
};

export const calculateVirtualRange = (
  rowCount: number,
  scrollTop: number,
  viewportHeight: number
): VirtualRange => {
  if (rowCount <= 0) {
    return {
      start: 0,
      end: 0,
      paddingTop: 0,
      paddingBottom: 0,
    };
  }

  const firstVisible = Math.floor(
    Math.max(0, scrollTop) / TREE_ROW_HEIGHT
  );
  const visibleRows = Math.max(
    1,
    Math.ceil(Math.max(0, viewportHeight) / TREE_ROW_HEIGHT)
  );
  const start = Math.max(0, firstVisible - TREE_ROW_OVERSCAN);
  const end = Math.min(
    rowCount,
    firstVisible + visibleRows + TREE_ROW_OVERSCAN
  );

  return {
    start,
    end,
    paddingTop: start * TREE_ROW_HEIGHT,
    paddingBottom: (rowCount - end) * TREE_ROW_HEIGHT,
  };
};
