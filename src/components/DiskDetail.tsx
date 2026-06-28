import { useEffect, useMemo, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent, ReactNode } from "react";
import { useLocation } from "react-router-dom";
import { invoke } from "@tauri-apps/api/tauri";
import { listen } from "@tauri-apps/api/event";
import { removeDir, removeFile } from "@tauri-apps/api/fs";
import diskIcon from "../assets/harddisk.png";
import duckIcon from "../assets/duck-scan.png";

type ScanStatus = {
  items: number;
  total: number;
  errors: number;
};

type ScanPhase =
  | "checkingCache"
  | "scanning"
  | "finalizing"
  | "preparing"
  | "rendering"
  | "failed";

type NodeStats = {
  items: number;
  files: number;
  folders: number;
  size: number;
};

type ExtensionStat = {
  extension: string;
  type: string;
  size: number;
  files: number;
};

type RouteState = {
  disk: string;
  used?: number;
  fullscan?: boolean;
};

type VisibleRow = {
  node: DiskItem;
  depth: number;
};

type DeleteState = {
  isDeleting: boolean;
  total: number;
  current: number;
  failed: number;
  error: string | null;
};

type DragSession = {
  node: DiskItem;
  startX: number;
  startY: number;
  x: number;
  y: number;
  active: boolean;
};

type DragPreview = {
  node: DiskItem;
  x: number;
  y: number;
};

const bytesFormatter = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 1,
});

const formatBytes = (bytes = 0) => {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }

  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const index = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1000)),
    units.length - 1
  );
  const value = bytes / Math.pow(1000, index);
  return `${bytesFormatter.format(value)} ${units[index]}`;
};

const getChildren = (node?: DiskItem | null) =>
  [...(node?.children || [])].sort((a, b) => (b.size || 0) - (a.size || 0));

const isDirectory = (node: DiskItem) =>
  node.isDirectory || Boolean(node.children && node.children.length > 0);

const getNodeName = (node: DiskItem) => {
  if (node.name && node.name !== "/") {
    return node.name;
  }

  return node.id || "/";
};

const getFileExtension = (name: string) => {
  const trimmed = name.trim();
  if (!trimmed || trimmed.startsWith(".") || !trimmed.includes(".")) {
    return "(no extension)";
  }

  const extension = trimmed.split(".").pop();
  return extension ? `.${extension.toLowerCase()}` : "(no extension)";
};

const getFileType = (extension: string) => {
  if (extension === "(no extension)") {
    return "No extension";
  }

  return `${extension.slice(1).toUpperCase()} file`;
};

const isDeletedPath = (id: string, deletedIds: Set<string>) => {
  if (deletedIds.has(id)) {
    return true;
  }

  for (const deletedId of deletedIds) {
    if (deletedId !== "/" && id.startsWith(`${deletedId}/`)) {
      return true;
    }
  }

  return false;
};

const colorForIndex = (index: number) => {
  const colors = [
    "#38bdf8",
    "#f59e0b",
    "#a78bfa",
    "#22c55e",
    "#f43f5e",
    "#14b8a6",
    "#eab308",
    "#fb7185",
    "#60a5fa",
    "#c084fc",
  ];
  return colors[index % colors.length];
};

const phaseLabel = (phase: ScanPhase, disk: string) => {
  switch (phase) {
    case "checkingCache":
      return `Checking cached scan for ${disk}`;
    case "scanning":
      return `Scanning ${disk}`;
    case "finalizing":
      return "Saving scan result";
    case "preparing":
      return "Preparing table data";
    case "rendering":
      return "Rendering table";
    case "failed":
      return "Scan failed";
  }
};

const emptyStats = { items: 0, files: 0, folders: 0, size: 0 };

const buildIndex = (root: DiskItem | null, deletedIds = new Set<string>()) => {
  const parentMap = new Map<string, DiskItem | null>();
  const statsMap = new Map<string, NodeStats>();

  const walk = (node: DiskItem, parent: DiskItem | null): NodeStats => {
    parentMap.set(node.id, parent);
    if (isDeletedPath(node.id, deletedIds)) {
      statsMap.set(node.id, emptyStats);
      return emptyStats;
    }

    const children = node.children || [];

    if (!children.length) {
      const stats = isDirectory(node)
        ? { items: 1, files: 0, folders: 1, size: node.size || 0 }
        : { items: 1, files: 1, folders: 0, size: node.size || 0 };
      statsMap.set(node.id, stats);
      return stats;
    }

    const childStats = children.map((child) => walk(child, node));
    const stats = childStats.reduce<NodeStats>(
      (acc, child) => ({
        items: acc.items + child.items,
        files: acc.files + child.files,
        folders: acc.folders + child.folders,
        size: acc.size + child.size,
      }),
      { items: 1, files: 0, folders: 1, size: 0 }
    );
    if (stats.size === 0 && (node.size || 0) > 0 && childStats.length === 0) {
      stats.size = node.size;
    }
    statsMap.set(node.id, stats);
    return stats;
  };

  if (root) {
    walk(root, null);
  }

  return { parentMap, statsMap };
};

const buildVisibleRows = (
  nodes: DiskItem[],
  expandedIds: Set<string>,
  depth = 0
): VisibleRow[] => {
  return nodes.flatMap((node) => {
    const row = { node, depth };
    if (!expandedIds.has(node.id)) {
      return [row];
    }

    return [row, ...buildVisibleRows(getChildren(node), expandedIds, depth + 1)];
  });
};

const buildExtensionStats = (node: DiskItem | null, deletedIds = new Set<string>()) => {
  const stats = new Map<string, ExtensionStat>();

  const walk = (item: DiskItem) => {
    if (isDeletedPath(item.id, deletedIds)) {
      return;
    }

    const children = item.children || [];
    if (children.length) {
      children.forEach(walk);
      return;
    }

    if (isDirectory(item)) {
      return;
    }

    const extension = getFileExtension(getNodeName(item));
    const current = stats.get(extension) || {
      extension,
      type: getFileType(extension),
      size: 0,
      files: 0,
    };
    current.size += item.size || 0;
    current.files += 1;
    stats.set(extension, current);
  };

  if (node) {
    walk(node);
  }

  return [...stats.values()].sort((a, b) => b.size - a.size).slice(0, 80);
};

const findNode = (root: DiskItem | null, id: string): DiskItem | null => {
  if (!root) {
    return null;
  }

  if (root.id === id) {
    return root;
  }

  for (const child of root.children || []) {
    const found = findNode(child, id);
    if (found) {
      return found;
    }
  }

  return null;
};

const removeNodes = (
  node: DiskItem | null,
  deletedIds: Set<string>
): DiskItem | null => {
  if (!node || deletedIds.has(node.id)) {
    return null;
  }

  return {
    ...node,
    children: (node.children || [])
      .map((child) => removeNodes(child, deletedIds))
      .filter(Boolean) as DiskItem[],
  };
};

const PercentBar = ({ percent }: { percent: number }) => (
  <div className="relative h-5 min-w-[76px] overflow-hidden rounded-sm border border-slate-700 bg-slate-950">
    <div
      className="absolute inset-y-0 left-0 bg-sky-500/70"
      style={{ width: `${Math.max(1, Math.min(100, percent))}%` }}
    />
    <div className="relative px-1 text-right text-xs tabular-nums text-slate-100">
      {percent.toFixed(1)}%
    </div>
  </div>
);

const TableHeader = ({ children }: { children: ReactNode }) => (
  <th className="sticky top-0 z-10 border-b border-slate-700 bg-slate-900 px-2 py-2 text-left text-xs font-semibold uppercase text-slate-400">
    {children}
  </th>
);

const NumberCell = ({ children }: { children: ReactNode }) => (
  <td className="whitespace-nowrap border-b border-slate-800 px-2 py-1.5 text-right tabular-nums text-slate-200">
    {children}
  </td>
);

const ScanningDuck = () => (
  <div className="duck-scan-stage" aria-hidden="true">
    <div className="duck-scan-disks">
      <img src={diskIcon} className="duck-scan-disk" />
      <img src={diskIcon} className="duck-scan-disk" />
      <img src={diskIcon} className="duck-scan-disk" />
    </div>
    <img src={duckIcon} className="duck-scan-duck" />
  </div>
);

const Scanning = () => {
  const location = useLocation() as { state?: RouteState };
  const { disk = "/", used = 0 } = location.state || {};
  const ratio = "0";

  const worker = useRef<Worker | null>(null);
  const dragSession = useRef<DragSession | null>(null);
  const dropZoneRef = useRef<HTMLDivElement | null>(null);
  const suppressClickUntil = useRef(0);
  const [view, setView] = useState<"loading" | "disk">("loading");
  const [status, setStatus] = useState<ScanStatus | null>(null);
  const [scanPhase, setScanPhase] = useState<ScanPhase>("checkingCache");
  const [scanError, setScanError] = useState<string | null>(null);
  const [rootNode, setRootNode] = useState<DiskItem | null>(null);
  const [currentNode, setCurrentNode] = useState<DiskItem | null>(null);
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());
  const [loadedFromCache, setLoadedFromCache] = useState(false);
  const [scanNonce, setScanNonce] = useState(0);
  const [deleteList, setDeleteList] = useState<DiskItem[]>([]);
  const [deletedIds, setDeletedIds] = useState<Set<string>>(new Set());
  const [dragPreview, setDragPreview] = useState<DragPreview | null>(null);
  const [isDeleteTargetActive, setDeleteTargetActive] = useState(false);
  const [deleteState, setDeleteState] = useState<DeleteState>({
    isDeleting: false,
    total: 0,
    current: 0,
    failed: 0,
    error: null,
  });

  useEffect(() => {
    let disposed = false;
    let scanningStarted = false;

    const unlistenStatus = listen("scan_status", (event: any) => {
      setStatus(event.payload as ScanStatus);
    });

    const unlistenFinalizing = listen("scan_finalizing", () => {
      setScanPhase("finalizing");
    });

    const unlistenFailed = listen("scan_failed", (event: any) => {
      setScanError(String(event.payload));
      setScanPhase("failed");
    });

    const unlistenCompleted = listen("scan_completed", async (event: any) => {
      try {
        setScanPhase("preparing");
        const payload = event.payload as { path: string };
        const scanResult = await invoke<string>("read_scan_result", {
          path: payload.path,
          scanPath: disk,
          ratio,
        });
        worker.current?.postMessage(scanResult);
      } catch (error) {
        setScanError(error instanceof Error ? error.message : String(error));
        setScanPhase("failed");
      }
    });

    worker.current = new Worker(
      new URL("../scanResult.worker.ts", import.meta.url),
      { type: "module" }
    );
    worker.current.onmessage = (
      event: MessageEvent<
        | { type: "done"; tree: DiskItem }
        | { type: "error"; message: string }
      >
    ) => {
      if (event.data.type === "error") {
        setScanError(event.data.message);
        setScanPhase("failed");
        return;
      }

      setScanPhase("rendering");
      setRootNode(event.data.tree);
      setCurrentNode(event.data.tree);
      setExpandedIds(new Set());
      setDeleteList([]);
      setView("disk");
    };

    const start = async () => {
      setView("loading");
      setStatus(null);
      setScanError(null);
      setRootNode(null);
      setCurrentNode(null);
      setDeleteList([]);
      setDeletedIds(new Set());
      setScanPhase(scanNonce === 0 ? "checkingCache" : "scanning");

      if (scanNonce === 0) {
        try {
          const cached = await invoke<string | null>("read_cached_scan_result", {
            scanPath: disk,
            ratio,
          });

          if (!disposed && cached) {
            setLoadedFromCache(true);
            setScanPhase("preparing");
            worker.current?.postMessage(cached);
            return;
          }
        } catch (error) {
          console.warn("Could not read cached scan result", error);
        }
      }

      if (disposed) {
        return;
      }

      setLoadedFromCache(false);
      setScanPhase("scanning");
      scanningStarted = true;
      invoke("start_scanning", { path: disk, ratio });
    };

    start();

    return () => {
      disposed = true;
      unlistenStatus.then((dispose) => dispose());
      unlistenFinalizing.then((dispose) => dispose());
      unlistenFailed.then((dispose) => dispose());
      unlistenCompleted.then((dispose) => dispose());
      worker.current?.terminate();
      if (scanningStarted) {
        invoke("stop_scanning", { path: disk });
      }
    };
  }, [disk, ratio, scanNonce]);

  useEffect(() => {
    const handlePointerMove = (event: PointerEvent) => {
      const session = dragSession.current;
      if (!session) {
        return;
      }

      const moved = Math.hypot(event.clientX - session.startX, event.clientY - session.startY);
      if (!session.active && moved < 5) {
        return;
      }

      session.active = true;
      session.x = event.clientX;
      session.y = event.clientY;
      setDragPreview({ node: session.node, x: event.clientX, y: event.clientY });

      const rect = dropZoneRef.current?.getBoundingClientRect();
      setDeleteTargetActive(
        Boolean(
          rect &&
            event.clientX >= rect.left &&
            event.clientX <= rect.right &&
            event.clientY >= rect.top &&
            event.clientY <= rect.bottom
        )
      );
    };

    const handlePointerUp = (event: PointerEvent) => {
      const session = dragSession.current;
      if (!session) {
        return;
      }

      if (session.active) {
        const rect = dropZoneRef.current?.getBoundingClientRect();
        if (
          rect &&
          event.clientX >= rect.left &&
          event.clientX <= rect.right &&
          event.clientY >= rect.top &&
          event.clientY <= rect.bottom
        ) {
          addDeleteTarget(session.node);
        }
        suppressClickUntil.current = Date.now() + 250;
      }

      dragSession.current = null;
      setDragPreview(null);
      setDeleteTargetActive(false);
    };

    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
    window.addEventListener("pointercancel", handlePointerUp);
    return () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerUp);
      window.removeEventListener("pointercancel", handlePointerUp);
    };
  }, [rootNode, deletedIds]);

  const { parentMap, statsMap: originalStatsMap } = useMemo(
    () => buildIndex(rootNode),
    [rootNode]
  );
  const { statsMap } = useMemo(
    () => buildIndex(rootNode, deletedIds),
    [rootNode, deletedIds]
  );
  const childRows = useMemo(() => getChildren(currentNode), [currentNode]);
  const rows = useMemo(
    () => buildVisibleRows(childRows, expandedIds),
    [childRows, expandedIds]
  );
  const extensionStats = useMemo(
    () => buildExtensionStats(currentNode, deletedIds),
    [currentNode, deletedIds]
  );
  const currentStats = currentNode
    ? statsMap.get(currentNode.id) || emptyStats
    : emptyStats;
  const parentNode = currentNode ? parentMap.get(currentNode.id) || null : null;
  const currentSize = currentStats.size;
  const rootSize = rootNode ? statsMap.get(rootNode.id)?.size || 0 : 0;
  const scannedTotal = status?.total || 0;
  const scanPercent =
    used > 0 ? Math.min(100, (Math.min(scannedTotal, used) / used) * 100) : 0;
  const topBlocks = childRows
    .filter((node) => !isDeletedPath(node.id, deletedIds))
    .slice(0, 20);

  const reveal = (node: DiskItem) => {
    invoke("show_in_folder", { path: node.id }).catch(console.error);
  };

  const toggleExpanded = (node: DiskItem) => {
    setExpandedIds((current) => {
      const next = new Set(current);
      if (next.has(node.id)) {
        next.delete(node.id);
      } else {
        next.add(node.id);
      }
      return next;
    });
  };

  const startRescan = () => {
    setLoadedFromCache(false);
    setScanNonce((current) => current + 1);
  };

  const addDeleteTarget = (node: DiskItem | null) => {
    if (!node || node.id === "/" || isDeletedPath(node.id, deletedIds)) {
      return;
    }

    setDeleteState((current) => ({
      ...current,
      error: null,
      failed: 0,
    }));
    setDeleteList((current) => {
      if (current.some((item) => item.id === node.id)) {
        return current;
      }
      return [...current, node];
    });
  };

  const startPointerDrag = (
    event: ReactPointerEvent<HTMLElement>,
    node: DiskItem,
    deleted: boolean
  ) => {
    if (event.button !== 0 || node.id === "/" || deleted) {
      return;
    }

    dragSession.current = {
      node,
      startX: event.clientX,
      startY: event.clientY,
      x: event.clientX,
      y: event.clientY,
      active: false,
    };
  };

  const deleteSelected = async () => {
    if (!deleteList.length) {
      return;
    }

    setDeleteState({
      isDeleting: true,
      total: deleteList.length,
      current: 0,
      failed: 0,
      error: null,
    });
    const successfulIds = new Set<string>();
    const failedItems: DiskItem[] = [];

    for (const node of deleteList) {
      try {
        await removeDir(node.id, { recursive: true }).catch(() =>
          removeFile(node.id)
        );
        successfulIds.add(node.id);
      } catch (error) {
        console.error(error);
        failedItems.push(node);
      } finally {
        setDeleteState((current) => ({
          ...current,
          current: current.current + 1,
        }));
      }
    }

    if (successfulIds.size) {
      setDeletedIds((current) => new Set([...current, ...successfulIds]));
      invoke("clear_cached_scan_result", { scanPath: disk, ratio }).catch(
        console.error
      );
    }

    setDeleteList(failedItems);
    setDeleteState({
      isDeleting: false,
      total: deleteList.length,
      current: deleteList.length,
      failed: failedItems.length,
      error: failedItems.length
        ? `Delete failed for ${failedItems.length} item${
            failedItems.length === 1 ? "" : "s"
          }`
        : null,
    });
  };

  if (view === "loading") {
    return (
      <div className="flex flex-1 flex-col items-center justify-center overflow-hidden">
        <ScanningDuck />
        <div className="w-2/3 max-w-2xl">
          <div className="mb-1 mt-5 text-center text-base font-medium text-white">
            {phaseLabel(scanPhase, disk)}
          </div>
          {scanError ? (
            <div className="mb-3 mt-1 text-center text-sm text-red-300">
              {scanError}
            </div>
          ) : (
            <div className="mb-3 mt-1 text-center text-sm text-slate-400">
              {status
                ? `${status.items.toLocaleString()} files - ${formatBytes(
                    status.total
                  )}${used > 0 ? ` - ${scanPercent.toFixed(1)}%` : ""}${
                    status.errors ? ` - ${status.errors} errors` : ""
                  }`
                : "Waiting for scan progress"}
            </div>
          )}
          <div className="h-3 w-full overflow-hidden rounded-full bg-slate-800">
            <div
              className="h-3 rounded-full bg-sky-500 progress-shimmer"
              style={{ width: `${status && used > 0 ? scanPercent : 100}%` }}
            />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col overflow-hidden bg-slate-950 text-sm text-slate-200">
      <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-4 border-b border-slate-800 bg-slate-900 px-3 py-2">
        <div className="min-w-0">
          <div className="flex items-center gap-2 text-xs text-slate-400">
            {parentNode && (
              <button
                onClick={() => {
                  setCurrentNode(parentNode);
                  setExpandedIds(new Set());
                }}
                className="rounded border border-slate-700 px-2 py-1 text-slate-200 hover:bg-slate-800"
              >
                Up
              </button>
            )}
            <span className="truncate">{currentNode?.id || disk}</span>
            {loadedFromCache && (
              <span className="rounded border border-emerald-700/60 px-2 py-0.5 text-emerald-300">
                Cached
              </span>
            )}
          </div>
          <div className="mt-2 grid grid-cols-5 gap-3">
            <div>
              <div className="text-xs text-slate-500">Selected</div>
              <div className="truncate text-base font-semibold text-white">
                {currentNode ? getNodeName(currentNode) : disk}
              </div>
            </div>
            <div>
              <div className="text-xs text-slate-500">Size</div>
              <div className="tabular-nums text-base font-semibold text-white">
                {formatBytes(currentSize)}
              </div>
            </div>
            <div>
              <div className="text-xs text-slate-500">Items</div>
              <div className="tabular-nums text-base font-semibold text-white">
                {currentStats.items.toLocaleString()}
              </div>
            </div>
            <div>
              <div className="text-xs text-slate-500">Files</div>
              <div className="tabular-nums text-base font-semibold text-white">
                {currentStats.files.toLocaleString()}
              </div>
            </div>
            <div>
              <div className="text-xs text-slate-500">Folders</div>
              <div className="tabular-nums text-base font-semibold text-white">
                {currentStats.folders.toLocaleString()}
              </div>
            </div>
          </div>
        </div>
        <div className="flex items-start gap-2">
          <button
            onClick={startRescan}
            className="rounded border border-slate-700 px-3 py-2 text-xs font-medium text-slate-100 hover:bg-slate-800"
          >
            Rescan
          </button>
          <button
            onClick={() => currentNode && reveal(currentNode)}
            className="rounded border border-slate-700 px-3 py-2 text-xs font-medium text-slate-100 hover:bg-slate-800"
          >
            Reveal
          </button>
        </div>
      </div>

      <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,2fr)_minmax(300px,0.85fr)] gap-px overflow-hidden bg-slate-800">
        <section className="flex min-w-0 min-h-0 flex-col overflow-hidden bg-slate-950">
          <div className="border-b border-slate-800 px-3 py-2 text-xs font-semibold uppercase text-slate-400">
            Tree View
          </div>
          <div className="min-h-0 flex-1 overflow-auto">
            <table className="w-full border-collapse text-xs">
              <thead>
                <tr>
                  <TableHeader>Name</TableHeader>
                  <TableHeader>Parent %</TableHeader>
                  <TableHeader>Size</TableHeader>
                  <TableHeader>Allocated</TableHeader>
                  <TableHeader>Items</TableHeader>
                  <TableHeader>Files</TableHeader>
                  <TableHeader>Folders</TableHeader>
                </tr>
              </thead>
              <tbody>
                {parentNode && (
                  <tr className="bg-slate-900/60 hover:bg-slate-800">
                    <td className="border-b border-slate-800 px-2 py-1.5 font-medium text-slate-100">
                      <button
                        onClick={() => {
                          setCurrentNode(parentNode);
                          setExpandedIds(new Set());
                        }}
                        className="text-slate-100 hover:underline"
                      >
                        ..
                      </button>
                    </td>
                    <td className="border-b border-slate-800 px-2 py-1.5" />
                    <NumberCell>
                      {formatBytes(statsMap.get(parentNode.id)?.size || 0)}
                    </NumberCell>
                    <NumberCell>
                      {formatBytes(statsMap.get(parentNode.id)?.size || 0)}
                    </NumberCell>
                    <NumberCell>
                      {(statsMap.get(parentNode.id)?.items || 0).toLocaleString()}
                    </NumberCell>
                    <NumberCell>
                      {(statsMap.get(parentNode.id)?.files || 0).toLocaleString()}
                    </NumberCell>
                    <NumberCell>
                      {(statsMap.get(parentNode.id)?.folders || 0).toLocaleString()}
                    </NumberCell>
                  </tr>
                )}
                {rows.map(({ node, depth }, index) => {
                  const deleted = isDeletedPath(node.id, deletedIds);
                  const effectiveStats = statsMap.get(node.id) || emptyStats;
                  const originalStats =
                    originalStatsMap.get(node.id) || {
                      items: 0,
                      files: 0,
                      folders: 0,
                      size: node.size || 0,
                    };
                  const stats = deleted ? originalStats : effectiveStats;
                  const parent = parentMap.get(node.id) || currentNode;
                  const parentEffectiveSize = parent
                    ? statsMap.get(parent.id)?.size || 0
                    : currentSize;
                  const parentOriginalSize = parent
                    ? originalStatsMap.get(parent.id)?.size || parent.size || 0
                    : currentSize;
                  const denominator = deleted
                    ? parentOriginalSize
                    : parentEffectiveSize;
                  const percent =
                    denominator > 0 ? ((stats.size || 0) / denominator) * 100 : 0;
                  const directory = isDirectory(node);
                  const expanded = expandedIds.has(node.id);
                  return (
                    <tr
                      key={node.id || `${node.name}-${index}`}
                      onPointerDown={(event) => startPointerDrag(event, node, deleted)}
                      onContextMenu={(event) => {
                        event.preventDefault();
                        reveal(node);
                      }}
                      style={
                        deleted
                          ? {
                              textDecoration: "line-through",
                              textDecorationColor: "#f87171",
                              textDecorationThickness: "2px",
                            }
                          : undefined
                      }
                      className={`select-none hover:bg-slate-900 ${
                        deleted ? "bg-red-950/20 text-red-300" : ""
                      }`}
                    >
                      <td
                        className={`max-w-[30rem] truncate border-b border-slate-800 px-2 py-1.5 ${
                          deleted
                            ? "text-red-300 line-through decoration-red-400 decoration-2"
                            : "text-slate-100"
                        }`}
                      >
                        <span
                          className="inline-block"
                          style={{ width: `${depth * 18}px` }}
                        />
                        {directory ? (
                          <button
                            disabled={deleted}
                            onPointerDown={(event) => event.stopPropagation()}
                            onClick={(event) => {
                              event.stopPropagation();
                              toggleExpanded(node);
                            }}
                            className="mr-2 inline-flex h-4 w-4 items-center justify-center rounded border border-slate-700 text-[11px] text-slate-300 hover:bg-slate-800 disabled:opacity-40"
                          >
                            {expanded ? "-" : "+"}
                          </button>
                        ) : (
                          <span className="mr-2 inline-block h-4 w-4" />
                        )}
                        <button
                          disabled={deleted}
                          onClick={() => {
                            if (Date.now() < suppressClickUntil.current) {
                              return;
                            }
                            if (directory) {
                              setCurrentNode(node);
                              setExpandedIds(new Set());
                            } else {
                              reveal(node);
                            }
                          }}
                          className={`text-left hover:underline disabled:hover:no-underline ${
                            deleted
                              ? "text-red-300 line-through decoration-red-400 decoration-2"
                              : "text-slate-100"
                          }`}
                        >
                          {getNodeName(node)}
                        </button>
                      </td>
                      <td className="border-b border-slate-800 px-2 py-1.5">
                        <PercentBar percent={percent} />
                      </td>
                      <NumberCell>{formatBytes(stats.size)}</NumberCell>
                      <NumberCell>{formatBytes(stats.size)}</NumberCell>
                      <NumberCell>{stats.items.toLocaleString()}</NumberCell>
                      <NumberCell>{stats.files.toLocaleString()}</NumberCell>
                      <NumberCell>{stats.folders.toLocaleString()}</NumberCell>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </section>

        <section className="flex min-w-0 min-h-0 flex-col overflow-hidden bg-slate-950">
          <div className="border-b border-slate-800 px-3 py-2 text-xs font-semibold uppercase text-slate-400">
            File Types
          </div>
          <div className="min-h-0 flex-1 overflow-auto">
            <table className="w-full border-collapse text-xs">
              <thead>
                <tr>
                  <TableHeader>Ext</TableHeader>
                  <TableHeader>Type</TableHeader>
                  <TableHeader>%</TableHeader>
                  <TableHeader>Size</TableHeader>
                  <TableHeader>Files</TableHeader>
                </tr>
              </thead>
              <tbody>
                {extensionStats.map((stat, index) => {
                  const percent =
                    currentSize > 0 ? (stat.size / currentSize) * 100 : 0;
                  return (
                    <tr key={stat.extension} className="hover:bg-slate-900">
                      <td className="whitespace-nowrap border-b border-slate-800 px-2 py-1.5 text-slate-100">
                        <span
                          className="mr-2 inline-block h-3 w-3 rounded-sm align-[-1px]"
                          style={{ backgroundColor: colorForIndex(index) }}
                        />
                        {stat.extension}
                      </td>
                      <td className="max-w-[12rem] truncate border-b border-slate-800 px-2 py-1.5 text-slate-300">
                        {stat.type}
                      </td>
                      <td className="border-b border-slate-800 px-2 py-1.5">
                        <PercentBar percent={percent} />
                      </td>
                      <NumberCell>{formatBytes(stat.size)}</NumberCell>
                      <NumberCell>{stat.files.toLocaleString()}</NumberCell>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </section>
      </div>

      <div className="grid grid-cols-[minmax(0,1fr)_360px] gap-px border-t border-slate-800 bg-slate-800">
        <div className="min-w-0 bg-slate-950 px-2 py-2">
          <div className="mb-1 flex items-center justify-between text-xs text-slate-500">
            <span className="truncate">{currentNode?.id || disk}</span>
            <span>{rootSize > 0 ? `${formatBytes(rootSize)} scanned` : ""}</span>
          </div>
          <div className="flex h-16 overflow-hidden rounded-sm border border-slate-800 bg-slate-900">
            {topBlocks.map((node, index) => {
              const blockSize = statsMap.get(node.id)?.size || 0;
              const width = currentSize > 0 ? (blockSize / currentSize) * 100 : 0;
              return (
                <button
                  key={node.id || `${node.name}-${index}`}
                  onClick={() => isDirectory(node) && setCurrentNode(node)}
                  className="group relative min-w-[28px] overflow-hidden border-r border-slate-950 text-left"
                  style={{
                    width: `${Math.max(1, width)}%`,
                    backgroundColor: colorForIndex(index),
                  }}
                  title={`${getNodeName(node)} - ${formatBytes(blockSize)}`}
                >
                  <span className="absolute left-1 top-1 max-w-[10rem] truncate text-[10px] font-semibold text-slate-950 group-hover:underline">
                    {getNodeName(node)}
                  </span>
                  <span className="absolute bottom-1 left-1 text-[10px] text-slate-950">
                  {formatBytes(blockSize)}
                </span>
              </button>
              );
            })}
          </div>
        </div>
        <div
          ref={dropZoneRef}
          className={`flex min-w-0 flex-col justify-between bg-slate-950 p-2 transition-colors ${
            isDeleteTargetActive ? "bg-red-950/40" : ""
          }`}
        >
          <div
            className={`min-h-[42px] rounded border border-dashed px-2 py-1.5 text-center text-xs ${
              isDeleteTargetActive
                ? "border-red-400 text-red-200"
                : "border-slate-600 text-slate-400"
            }`}
          >
            {deleteList.length === 0 ? (
              "Drag files or folders here to delete"
            ) : (
              <div className="truncate text-left text-slate-200">
                {deleteList.length} selected:{" "}
                {deleteList.map((item) => getNodeName(item)).join(", ")}
              </div>
            )}
          </div>
          {(deleteState.isDeleting || deleteState.error) && (
            <div className="mt-2">
              <div className="mb-1 flex items-center justify-between text-[11px] text-slate-400">
                <span
                  className={
                    deleteState.error ? "font-medium text-red-300" : ""
                  }
                >
                  {deleteState.error || "Deleting"}
                </span>
                <span>
                  {deleteState.current}/{deleteState.total}
                </span>
              </div>
              <div className="h-2 overflow-hidden rounded bg-slate-800">
                <div
                  className={`h-2 rounded ${
                    deleteState.error ? "bg-red-500" : "bg-sky-500"
                  }`}
                  style={{
                    width: `${
                      deleteState.total > 0
                        ? Math.max(
                            4,
                            (deleteState.current / deleteState.total) * 100
                          )
                        : 0
                    }%`,
                  }}
                />
              </div>
            </div>
          )}
          <div className="mt-2 flex gap-2">
            <button
              onClick={() => {
                setDeleteList([]);
                setDeleteState({
                  isDeleting: false,
                  total: 0,
                  current: 0,
                  failed: 0,
                  error: null,
                });
              }}
              disabled={!deleteList.length || deleteState.isDeleting}
              className="flex-1 rounded border border-slate-700 px-3 py-1.5 text-xs text-slate-200 disabled:cursor-not-allowed disabled:opacity-40"
            >
              Clear
            </button>
            <button
              onClick={deleteSelected}
              disabled={!deleteList.length || deleteState.isDeleting}
              className="flex-1 rounded bg-red-700 px-3 py-1.5 text-xs font-medium text-white disabled:cursor-not-allowed disabled:opacity-40"
            >
              {deleteState.isDeleting
                ? `Deleting ${deleteState.current}/${deleteState.total}`
                : "Delete"}
            </button>
          </div>
        </div>
      </div>
      {dragPreview && (
        <div
          className="pointer-events-none fixed z-50 max-w-[280px] truncate rounded border border-red-400/80 bg-red-950/90 px-3 py-1.5 text-xs font-medium text-red-100 shadow-lg"
          style={{
            left: dragPreview.x + 12,
            top: dragPreview.y + 12,
          }}
        >
          {getNodeName(dragPreview.node)}
        </div>
      )}
    </div>
  );
};

export default Scanning;
