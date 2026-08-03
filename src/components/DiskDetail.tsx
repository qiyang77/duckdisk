import { useEffect, useMemo, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent, ReactNode } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/tauri";
import { listen } from "@tauri-apps/api/event";
import * as d3 from "d3";
import surfingDuck from "../assets/duck-disc-surf.png";
import { formatBytes } from "../formatBytes";
import {
  type DiskRouteState,
  readDiskRoute,
  rememberDiskRoute,
} from "../diskRoute";
import {
  calculateVirtualRange,
  TREE_ROW_HEIGHT,
} from "../virtualRows";
import {
  AlertTriangle,
  ArrowLeft,
  ArrowUp,
  ChevronDown,
  ChevronRight,
  File as FileIcon,
  Folder as FolderIcon,
  FolderOpen,
  FolderUp,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
  Square,
  Trash2,
  X,
} from "lucide-react";

type ScanStatus = {
  items: number;
  total: number;
  operationNotPermitted: number;
  permissionDenied: number;
  interrupted: number;
  other: number;
};

type ScanPhase =
  | "checkingCache"
  | "scanning"
  | "incremental"
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

type TreemapDatum = {
  node?: DiskItem;
  size?: number;
  children?: TreemapDatum[];
};

type TreemapBlock = {
  node: DiskItem;
  index: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
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

type CloudDeleteResult = {
  deletedIds: string[];
  failures: Array<{
    itemId: string;
    message: string;
  }>;
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

type ContextMenuState = {
  node: DiskItem;
  x: number;
  y: number;
};

type RefreshNotice = {
  kind: "success" | "error";
  message: string;
};

type ScanErrorCounts = {
  operationNotPermitted: number;
  permissionDenied: number;
  interrupted: number;
  other: number;
};

type ScanErrorRecord = {
  operation: string;
  path: string;
  reason: string;
  kind: keyof ScanErrorCounts;
};

type ScanErrorReport = {
  counts: ScanErrorCounts;
  records: ScanErrorRecord[];
};

const getChildren = (node?: DiskItem | null) => node?.children || [];

const sortChildrenBySize = (children: DiskItem[]) =>
  children.sort((a, b) => (b.size || 0) - (a.size || 0));

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

const treemapColorForIndex = (index: number) => {
  const colors = [
    "#309fc5",
    "#d18a35",
    "#8b72ce",
    "#48a374",
    "#c85c66",
    "#3d9992",
    "#b99a38",
    "#bb6f98",
    "#4f86c4",
    "#876fb4",
  ];
  return colors[index % colors.length];
};

const phaseLabel = (phase: ScanPhase, disk: string) => {
  switch (phase) {
    case "checkingCache":
      return `Checking cached scan for ${disk}`;
    case "scanning":
      return `Scanning ${disk}`;
    case "incremental":
      return `Updating cached scan for ${disk}`;
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
const emptyScanErrorCounts = {
  operationNotPermitted: 0,
  permissionDenied: 0,
  interrupted: 0,
  other: 0,
};
const emptyScanErrorReport = {
  counts: emptyScanErrorCounts,
  records: [],
};
const credentialNoticeKey = (source: string) =>
  `duckdisk-${source}-keychain-notice-seen`;

const totalScanIssues = (counts: ScanErrorCounts) =>
  counts.operationNotPermitted +
  counts.permissionDenied +
  counts.interrupted +
  counts.other;

const hasPermissionIssues = (counts: ScanErrorCounts) =>
  counts.operationNotPermitted > 0 || counts.permissionDenied > 0;

const formatScanIssueCounts = (counts: ScanErrorCounts) => {
  const parts = [
    `not permitted ${counts.operationNotPermitted}`,
    `denied ${counts.permissionDenied}`,
    `interrupted ${counts.interrupted}`,
  ];

  if (counts.other) {
    parts.push(`other ${counts.other}`);
  }

  return parts.join(" - ");
};

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
  expandedIds: Set<string>
): VisibleRow[] => {
  const rows: VisibleRow[] = [];
  const stack: VisibleRow[] = [];

  for (let index = nodes.length - 1; index >= 0; index -= 1) {
    stack.push({ node: nodes[index], depth: 0 });
  }

  while (stack.length) {
    const row = stack.pop()!;
    rows.push(row);
    if (!expandedIds.has(row.node.id)) {
      continue;
    }

    const children = getChildren(row.node);
    for (let index = children.length - 1; index >= 0; index -= 1) {
      stack.push({ node: children[index], depth: row.depth + 1 });
    }
  }

  return rows;
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

const replaceTreeNode = (
  root: DiskItem,
  nodeId: string,
  replacement: DiskItem
): DiskItem => {
  if (root.id === nodeId) {
    return replacement;
  }

  return {
    ...root,
    children: sortChildrenBySize(
      (root.children || []).map((child) =>
        child.id === nodeId
          ? replacement
          : nodeId.startsWith(`${child.id}/`)
          ? replaceTreeNode(child, nodeId, replacement)
          : child
      )
    ),
  };
};

const childNodeId = (parentId: string, name: string) =>
  parentId === "/"
    ? `/${name.replace(/^\/+/, "")}`
    : `${parentId}/${name.replace(/^\/+/, "")}`;

const mapRefreshedTree = (raw: any, original: DiskItem): DiskItem => {
  const walk = (item: any, id: string, isRoot = false): DiskItem => {
    const children = Array.isArray(item.children)
      ? item.children.map((child: any) =>
          walk(child, childNodeId(id, String(child.name || "(unnamed)")))
        )
      : [];
    sortChildrenBySize(children);
    const size = Number(item.size || 0);
    return {
      ...item,
      id,
      name: isRoot ? original.name : String(item.name || "(unnamed)"),
      value: size,
      size,
      isDirectory: isRoot
        ? isDirectory(original)
        : Boolean(item.isDirectory || children.length),
      children,
    };
  };

  return walk(raw, original.id, true);
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
  <div className="data-percent">
    <div
      className="data-percent-fill"
      style={{ width: `${Math.max(1, Math.min(100, percent))}%` }}
    />
    <div className="data-percent-label">
      {percent.toFixed(1)}%
    </div>
  </div>
);

const TableHeader = ({ children }: { children: ReactNode }) => (
  <th className="data-header">
    {children}
  </th>
);

const NumberCell = ({ children }: { children: ReactNode }) => (
  <td className="data-cell data-cell-number">
    {children}
  </td>
);

const ScanningDuck = () => (
  <div className="duck-scan-stage" aria-hidden="true">
    <div className="duck-scan-trail duck-scan-trail-back" />
    <div className="duck-scan-trail duck-scan-trail-front" />
    <div className="duck-file-wake">
      <span className="duck-file-splash duck-file-splash-one" />
      <span className="duck-file-splash duck-file-splash-two" />
      <span className="duck-file-splash duck-file-splash-three" />
    </div>
    <div className="duck-scan-surfer">
      <img src={surfingDuck} alt="" />
    </div>
  </div>
);

const Scanning = () => {
  const location = useLocation() as { state?: DiskRouteState };
  const navigate = useNavigate();
  const routeState = location.state || readDiskRoute() || undefined;
  const {
    disk = "/",
    used = 0,
    source = "local",
    accountId = "",
  } = routeState || {};
  const isOneDrive = source === "onedrive";
  const isGoogleDrive = source === "googledrive";
  const isSsh = source === "ssh";
  const isCloud = source !== "local";
  const requiresKeychainApproval = isOneDrive || isGoogleDrive;
  const canDelete = true;
  const usesTrash = isOneDrive || isGoogleDrive;
  const canRefreshItem = !isCloud || isOneDrive;
  const providerName = isOneDrive
    ? "OneDrive"
    : isGoogleDrive
    ? "Google Drive"
    : isSsh
    ? "SSH"
    : "Local disk";
  const cloudCommandPrefix = isGoogleDrive
    ? "google_drive"
    : isSsh
    ? "ssh"
    : "onedrive";
  const cloudEventPrefix = isGoogleDrive
    ? "googledrive"
    : isSsh
    ? "ssh"
    : "onedrive";
  const cloudTrashName = isGoogleDrive
    ? "Google Drive Trash"
    : "OneDrive Recycle Bin";
  const ratio = "0";

  const worker = useRef<Worker | null>(null);
  const dragSession = useRef<DragSession | null>(null);
  const dropZoneRef = useRef<HTMLDivElement | null>(null);
  const treeViewportRef = useRef<HTMLDivElement | null>(null);
  const treeScrollFrameRef = useRef<number | null>(null);
  const suppressClickUntil = useRef(0);
  const [view, setView] = useState<"loading" | "disk">("loading");
  const [status, setStatus] = useState<ScanStatus | null>(null);
  const [scanPhase, setScanPhase] = useState<ScanPhase>("checkingCache");
  const [scanError, setScanError] = useState<string | null>(null);
  const [scanIssueReport, setScanIssueReport] =
    useState<ScanErrorReport>(emptyScanErrorReport);
  const [showScanIssues, setShowScanIssues] = useState(false);
  const [rootNode, setRootNode] = useState<DiskItem | null>(null);
  const [currentNode, setCurrentNode] = useState<DiskItem | null>(null);
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());
  const [loadedFromCache, setLoadedFromCache] = useState(false);
  const [scanNonce, setScanNonce] = useState(0);
  const [isStoppingScan, setStoppingScan] = useState(false);
  const [oneDriveKeychainApproved, setOneDriveKeychainApproved] = useState(
    () =>
      !requiresKeychainApproval ||
      sessionStorage.getItem(credentialNoticeKey(source)) === "true"
  );
  const [deleteList, setDeleteList] = useState<DiskItem[]>([]);
  const [showPermanentDeleteConfirmation, setShowPermanentDeleteConfirmation] =
    useState(false);
  const [deletedIds, setDeletedIds] = useState<Set<string>>(new Set());
  const [dragPreview, setDragPreview] = useState<DragPreview | null>(null);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [refreshingNodeId, setRefreshingNodeId] = useState<string | null>(null);
  const [refreshNotice, setRefreshNotice] = useState<RefreshNotice | null>(null);
  const [isDeleteTargetActive, setDeleteTargetActive] = useState(false);
  const [treeViewport, setTreeViewport] = useState({
    scrollTop: 0,
    height: 0,
  });
  const [deleteState, setDeleteState] = useState<DeleteState>({
    isDeleting: false,
    total: 0,
    current: 0,
    failed: 0,
    error: null,
  });

  useEffect(() => {
    if (routeState?.disk) {
      rememberDiskRoute(routeState);
    }
  }, [routeState]);

  useEffect(() => {
    const viewport = treeViewportRef.current;
    if (!viewport || view !== "disk") {
      return;
    }

    const updateViewport = () => {
      setTreeViewport({
        scrollTop: viewport.scrollTop,
        height: viewport.clientHeight,
      });
    };
    updateViewport();

    const observer = new ResizeObserver(updateViewport);
    observer.observe(viewport);
    return () => observer.disconnect();
  }, [view]);

  useEffect(
    () => () => {
      if (treeScrollFrameRef.current !== null) {
        cancelAnimationFrame(treeScrollFrameRef.current);
      }
    },
    []
  );

  useEffect(() => {
    const viewport = treeViewportRef.current;
    if (!viewport) {
      return;
    }

    viewport.scrollTop = 0;
    setTreeViewport((current) => ({ ...current, scrollTop: 0 }));
  }, [currentNode?.id]);

  useEffect(() => {
    const preventNativeContextMenu = (event: MouseEvent) => {
      event.preventDefault();
    };
    const closeContextMenu = () => setContextMenu(null);
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        closeContextMenu();
      }
    };

    window.addEventListener("contextmenu", preventNativeContextMenu);
    window.addEventListener("pointerdown", closeContextMenu);
    window.addEventListener("resize", closeContextMenu);
    window.addEventListener("scroll", closeContextMenu, true);
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("contextmenu", preventNativeContextMenu);
      window.removeEventListener("pointerdown", closeContextMenu);
      window.removeEventListener("resize", closeContextMenu);
      window.removeEventListener("scroll", closeContextMenu, true);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let scanningStarted = false;

    const cloudEventMatches = (event: any) =>
      !isCloud || event.payload?.accountId === accountId;
    const eventName = (name: string) =>
      isCloud ? `${cloudEventPrefix}_${name}` : name;

    const unlistenStatus = listen(eventName("scan_status"), (event: any) => {
      if (!cloudEventMatches(event)) return;
      setStatus(event.payload as ScanStatus);
    });

    const unlistenFinalizing = listen(eventName("scan_finalizing"), (event) => {
      if (!cloudEventMatches(event)) return;
      setScanPhase("finalizing");
    });

    const unlistenIncremental = listen(eventName("scan_incremental"), (event) => {
      if (!cloudEventMatches(event)) return;
      setLoadedFromCache(true);
      setScanPhase("incremental");
    });

    const unlistenFull = listen(eventName("scan_full"), (event) => {
      if (!cloudEventMatches(event)) return;
      setLoadedFromCache(false);
      setScanPhase("scanning");
    });

    const unlistenFailed = !isCloud
      ? listen("scan_failed", (event: any) => {
          setScanError(String(event.payload));
          setScanPhase("failed");
        })
      : Promise.resolve(() => {});

    const unlistenCloudFailed = isCloud
      ? listen(`${cloudEventPrefix}_scan_failed`, (event: any) => {
          if (!cloudEventMatches(event)) return;
          setScanError(String(event.payload.message));
          setScanPhase("failed");
        })
      : Promise.resolve(() => {});

    const unlistenCompleted = listen(eventName("scan_completed"), async (event: any) => {
      if (!cloudEventMatches(event)) return;
      try {
        setScanPhase("preparing");
        const payload = event.payload as { path: string; errorsPath: string };
        if (!isCloud && payload.errorsPath) {
          const errorReport = await invoke<string>("read_scan_error_report", {
            path: payload.errorsPath,
          });
          setScanIssueReport(JSON.parse(errorReport));
        } else {
          setScanIssueReport(emptyScanErrorReport);
        }
        const scanResult = isCloud
          ? await invoke<string>(`read_${cloudCommandPrefix}_scan_result`, {
              path: payload.path,
            })
          : await invoke<string>("read_scan_result", {
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

    const unlistenDeleteStatus = listen(
      `${cloudEventPrefix}_delete_status`,
      (event: any) => {
        const payload = event.payload as { current: number; total: number };
        setDeleteState((current) => ({
          ...current,
          current: payload.current,
          total: payload.total,
        }));
      }
    );

    const listenersReady = Promise.all([
      unlistenStatus,
      unlistenFinalizing,
      unlistenIncremental,
      unlistenFull,
      unlistenFailed,
      unlistenCloudFailed,
      unlistenCompleted,
      unlistenDeleteStatus,
    ]);

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
      setScanIssueReport(emptyScanErrorReport);
      setShowScanIssues(false);
      setDeleteList([]);
      setDeletedIds(new Set());
      setScanPhase(scanNonce === 0 ? "checkingCache" : "scanning");

      // Cached cloud and SSH scans can emit their completion events before
      // invoke() resolves, so every listener must be active before starting.
      await listenersReady;
      if (disposed) {
        return;
      }

      if (isCloud) {
        if (!accountId) {
          setScanError(`${providerName} connection information is missing`);
          setScanPhase("failed");
          return;
        }
        if (requiresKeychainApproval && !oneDriveKeychainApproved) {
          return;
        }
        setLoadedFromCache(false);
        setScanPhase(scanNonce === 0 ? "checkingCache" : "scanning");
        scanningStarted = true;
        const command = `start_${cloudCommandPrefix}_scan`;
        const args = isSsh
          ? { connectionId: accountId, forceFull: scanNonce > 0 }
          : { accountId, forceFull: scanNonce > 0 };
        invoke(command, args).catch((error) => {
          setScanError(String(error));
          setScanPhase("failed");
        });
        return;
      }

      if (scanNonce === 0) {
        try {
          const cached = await invoke<string | null>("read_cached_scan_result", {
            scanPath: disk,
            ratio,
          });

          if (!disposed && cached) {
            const hasIndex = await invoke<boolean>("has_cached_scan_index", {
              scanPath: disk,
              ratio,
            });

            if (hasIndex) {
              setLoadedFromCache(true);
              setScanIssueReport(emptyScanErrorReport);
              setScanPhase("incremental");
              scanningStarted = true;
              invoke("start_scanning", { path: disk, ratio, useCache: true });
              return;
            }
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
      invoke("start_scanning", { path: disk, ratio, useCache: false });
    };

    start();

    return () => {
      disposed = true;
      unlistenStatus.then((dispose) => dispose());
      unlistenFinalizing.then((dispose) => dispose());
      unlistenIncremental.then((dispose) => dispose());
      unlistenFull.then((dispose) => dispose());
      unlistenFailed.then((dispose) => dispose());
      unlistenCloudFailed.then((dispose) => dispose());
      unlistenCompleted.then((dispose) => dispose());
      unlistenDeleteStatus.then((dispose) => dispose());
      worker.current?.terminate();
      if (scanningStarted) {
        if (isCloud) {
          const command = `stop_${cloudCommandPrefix}_scan`;
          const args = isSsh
            ? { connectionId: accountId }
            : { accountId };
          invoke(command, args).catch(console.error);
        } else {
          invoke("stop_scanning", { path: disk }).catch(console.error);
        }
      }
    };
  }, [
    accountId,
    cloudCommandPrefix,
    cloudEventPrefix,
    disk,
    isCloud,
    isSsh,
    oneDriveKeychainApproved,
    providerName,
    ratio,
    requiresKeychainApproval,
    scanNonce,
  ]);

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
  const virtualRange = useMemo(
    () =>
      calculateVirtualRange(
        rows.length,
        treeViewport.scrollTop,
        treeViewport.height
      ),
    [rows.length, treeViewport]
  );
  const virtualRows = useMemo(
    () => rows.slice(virtualRange.start, virtualRange.end),
    [rows, virtualRange.start, virtualRange.end]
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
  const issueCount = totalScanIssues(scanIssueReport.counts);
  const canOpenScanIssues =
    !isCloud && (issueCount > 0 || loadedFromCache);
  const scanPercent =
    used > 0 ? Math.min(100, (Math.min(scannedTotal, used) / used) * 100) : 0;
  const hasDeterminateProgress =
    !isCloud && scanPhase === "scanning" && status !== null && used > 0;
  const treemapBlocks = useMemo<TreemapBlock[]>(() => {
    const candidates = childRows
      .filter((node) => !isDeletedPath(node.id, deletedIds))
      .map((node) => ({
        node,
        size: statsMap.get(node.id)?.size || 0,
      }))
      .filter((item) => item.size > 0)
      .slice(0, 48);

    if (!candidates.length || currentSize <= 0) {
      return [];
    }

    const root = d3
      .hierarchy<TreemapDatum>({
        children: candidates.map(({ node, size }) => ({ node, size })),
      })
      .sum((item) => item.size || 0)
      .sort((a, b) => (b.value || 0) - (a.value || 0));

    const layout = d3
      .treemap<TreemapDatum>()
      .tile(d3.treemapSquarify.ratio(1.25))
      .size([1000, 108])
      .paddingInner(2)
      .paddingOuter(2)
      .round(true)(root);

    return layout.leaves().map((leaf, index) => ({
      node: leaf.data.node!,
      index,
      x0: leaf.x0,
      y0: leaf.y0,
      x1: leaf.x1,
      y1: leaf.y1,
    }));
  }, [childRows, currentSize, deletedIds, statsMap]);
  const treeColumnCount = isCloud ? 6 : 7;

  const handleTreeScroll = () => {
    if (treeScrollFrameRef.current !== null) {
      return;
    }

    treeScrollFrameRef.current = requestAnimationFrame(() => {
      treeScrollFrameRef.current = null;
      const viewport = treeViewportRef.current;
      if (!viewport) {
        return;
      }
      setTreeViewport({
        scrollTop: viewport.scrollTop,
        height: viewport.clientHeight,
      });
    });
  };

  const reveal = (node: DiskItem) => {
    if (isCloud) {
      return;
    }
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

  const startRescan = async () => {
    if (!isCloud && loadedFromCache) {
      await invoke("clear_cached_scan_result", { scanPath: disk, ratio }).catch(
        console.error
      );
    }
    if (isSsh && loadedFromCache) {
      await invoke("clear_ssh_cached_scan_result", {
        connectionId: accountId,
      }).catch(console.error);
    }
    setLoadedFromCache(false);
    setScanNonce((current) => current + 1);
  };

  const stopScanAndReturn = async () => {
    setStoppingScan(true);
    try {
      if (isCloud) {
        await invoke(`stop_${cloudCommandPrefix}_scan`,
          isSsh ? { connectionId: accountId } : { accountId }
        );
      } else {
        await invoke("stop_scanning", { path: disk });
      }
    } catch (error) {
      console.error(error);
    } finally {
      navigate("/");
    }
  };

  const addDeleteTarget = (node: DiskItem | null) => {
    if (
      !canDelete ||
      !node ||
      node.id === "/" ||
      node.id === rootNode?.id ||
      isDeletedPath(node.id, deletedIds)
    ) {
      return;
    }

    setDeleteState((current) => ({
      ...current,
      error: null,
      failed: 0,
    }));
    setDeleteList((current) => {
      if (
        current.some(
          (item) => item.id === node.id || node.id.startsWith(`${item.id}/`)
        )
      ) {
        return current;
      }
      return [
        ...current.filter((item) => !item.id.startsWith(`${node.id}/`)),
        node,
      ];
    });
  };

  const refreshNode = async (node: DiskItem) => {
    if (refreshingNodeId) {
      return;
    }

    setContextMenu(null);
    setRefreshNotice(null);
    setRefreshingNodeId(node.id);
    try {
      if (!canRefreshItem) {
        throw new Error(`Use Rescan to update ${providerName} results`);
      }
      if (isCloud && !node.cloudId) {
        throw new Error("The selected OneDrive item has no cloud identifier");
      }
      const content = isCloud
        ? await invoke<string>("refresh_onedrive_item", {
            accountId,
            itemId: node.cloudId,
          })
        : await invoke<string>("refresh_scan_path", {
            scanPath: disk,
            targetPath: node.id,
            ratio,
          });
      const parsed = JSON.parse(content);
      const refreshed = mapRefreshedTree(
        isCloud ? parsed : parsed.tree,
        node
      );
      if (!rootNode) {
        throw new Error("The scan tree is no longer available");
      }

      const nextRoot = replaceTreeNode(rootNode, node.id, refreshed);
      setRootNode(nextRoot);
      setCurrentNode((current) =>
        current ? findNode(nextRoot, current.id) || nextRoot : nextRoot
      );
      setDeleteList((current) =>
        current.map((item) => findNode(nextRoot, item.id) || item)
      );
      setRefreshNotice({
        kind: "success",
        message: `${getNodeName(node)} updated`,
      });
    } catch (error) {
      setRefreshNotice({
        kind: "error",
        message: String(error),
      });
    } finally {
      setRefreshingNodeId(null);
    }
  };

  const startPointerDrag = (
    event: ReactPointerEvent<HTMLElement>,
    node: DiskItem,
    deleted: boolean
  ) => {
    if (
      !canDelete ||
      event.button !== 0 ||
      node.id === "/" ||
      node.id === rootNode?.id ||
      deleted
    ) {
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
    let cloudFailureMessage: string | null = null;

    if (isOneDrive || isGoogleDrive || isSsh) {
      try {
        const result = await invoke<CloudDeleteResult>(
          isSsh
            ? "delete_ssh_items"
            : isGoogleDrive
            ? "delete_google_drive_items"
            : "delete_onedrive_items",
          isSsh
            ? {
                connectionId: accountId,
                itemIds: deleteList
                  .map((node) => node.cloudId)
                  .filter((itemId): itemId is string => Boolean(itemId)),
              }
            : {
                accountId,
                itemIds: deleteList
                  .map((node) => node.cloudId)
                  .filter((itemId): itemId is string => Boolean(itemId)),
              }
        );
        cloudFailureMessage = result.failures[0]?.message || null;
        const deletedCloudIds = new Set(result.deletedIds);
        for (const node of deleteList) {
          if (node.cloudId && deletedCloudIds.has(node.cloudId)) {
            successfulIds.add(node.id);
          } else {
            failedItems.push(node);
          }
        }
      } catch (error) {
        console.error(error);
        failedItems.push(...deleteList);
        setDeleteList(deleteList);
        setDeleteState({
          isDeleting: false,
          total: deleteList.length,
          current: 0,
          failed: deleteList.length,
          error: String(error),
        });
        return;
      }
    } else {
      for (const node of deleteList) {
        try {
          await invoke("delete_local_item", {
            scanRoot: disk,
            itemPath: node.id,
          });
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
    }

    if (successfulIds.size) {
      setDeletedIds((current) => new Set([...current, ...successfulIds]));
      if (!isCloud) {
        invoke("clear_cached_scan_result", { scanPath: disk, ratio }).catch(
          console.error
        );
      }
    }

    setDeleteList(failedItems);
    const compactCloudFailure = cloudFailureMessage
      ?.replace(/\s+/g, " ")
      .slice(0, 220);
    setDeleteState({
      isDeleting: false,
      total: deleteList.length,
      current: deleteList.length,
      failed: failedItems.length,
      error: failedItems.length
        ? `${usesTrash ? "Move failed" : "Delete failed"} for ${
            failedItems.length
          } item${
            failedItems.length === 1 ? "" : "s"
          }${compactCloudFailure ? `: ${compactCloudFailure}` : ""}`
        : null,
    });
  };

  if (
    view === "loading" &&
    requiresKeychainApproval &&
    !oneDriveKeychainApproved
  ) {
    return (
      <div className="dialog-stage">
        <div
          role="dialog"
          aria-modal="true"
          aria-labelledby="cloud-keychain-title"
          className="app-dialog w-full max-w-lg p-6"
        >
          <div className="dialog-eyebrow">
            {providerName} security
          </div>
          <h1
            id="cloud-keychain-title"
            className="dialog-title"
          >
            Allow access to your saved sign-in
          </h1>
          <p className="dialog-copy">
            DuckDisk stores your {providerName} sign-in token in macOS Keychain.
            To refresh this scan, DuckDisk needs to read that saved token.
          </p>
          <p className="dialog-copy dialog-copy-muted">
            macOS may ask for your Mac login password. The password is handled
            by macOS and is never received or stored by DuckDisk.
          </p>
          <div className="mt-6 flex justify-end gap-3">
            <button
              onClick={() => navigate("/")}
              className="button button-secondary"
            >
              Not Now
            </button>
            <button
              autoFocus
              onClick={() => {
                sessionStorage.setItem(credentialNoticeKey(source), "true");
                setOneDriveKeychainApproved(true);
              }}
              className="button button-primary"
            >
              Continue
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (view === "loading") {
    return (
      <div className="scan-stage">
        <div className="scan-visual">
          <ScanningDuck />
        </div>
        <div className="scan-status">
          <div className="scan-title">
            {phaseLabel(scanPhase, disk)}
          </div>
          {scanError ? (
            <div className="scan-message scan-message-error">
              {scanError}
            </div>
          ) : (
            <div className="scan-message">
              {status
                ? isCloud
                  ? `${status.items.toLocaleString()} ${
                      isSsh ? "remote" : "cloud"
                    } items scanned${
                      status.total ? ` - ${formatBytes(status.total)}` : ""
                    }`
                  : `${status.items.toLocaleString()} ${
                      scanPhase === "incremental" ? "files checked" : "files"
                    } - ${formatBytes(status.total)}${
                      scanPhase === "incremental" ? " rescanned" : ""
                    }${hasDeterminateProgress ? ` - ${scanPercent.toFixed(1)}%` : ""}${
                    totalScanIssues(status) ? ` - ${formatScanIssueCounts(status)}` : ""
                  }`
                : "Waiting for scan progress"}
            </div>
          )}
          <div className="scan-progress-track">
            <div
              className={`scan-progress-fill ${
                hasDeterminateProgress
                  ? "progress-shimmer"
                  : "progress-indeterminate"
              }`}
              style={{
                width: `${hasDeterminateProgress ? scanPercent : 32}%`,
              }}
            />
          </div>
          {scanPhase === "failed" ? (
            <div className="scan-actions">
              <button
                type="button"
                className="button button-secondary"
                onClick={() => navigate("/")}
              >
                <ArrowLeft size={14} />
                Back to All Disks
              </button>
            </div>
          ) : (
            <div className="scan-actions">
              <button
                type="button"
                className="button button-secondary button-cancel"
                onClick={stopScanAndReturn}
                disabled={isStoppingScan}
              >
                {isStoppingScan ? (
                  <RefreshCw size={14} className="animate-spin" />
                ) : (
                  <Square size={12} fill="currentColor" />
                )}
                {isStoppingScan ? "Stopping..." : "Stop & Back"}
              </button>
            </div>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="results-workspace">
      <div className="results-summary">
        <div className="min-w-0">
          <div className="results-pathbar">
            {parentNode && (
              <button
                type="button"
                title="Go to parent folder"
                onClick={() => {
                  setCurrentNode(parentNode);
                  setExpandedIds(new Set());
                }}
                className="button button-secondary h-[26px] min-h-0 px-2"
              >
                <ArrowUp size={13} />
                Up
              </button>
            )}
            <span className="truncate text-[#89949e]">{currentNode?.id || disk}</span>
            {refreshingNodeId && (
              <span className="inline-flex shrink-0 items-center gap-1.5 text-[#62c2e3]">
                <RefreshCw size={12} className="animate-spin" />
                Refreshing {getNodeName(findNode(rootNode, refreshingNodeId) || ({ id: refreshingNodeId } as DiskItem))}
              </span>
            )}
            {!refreshingNodeId && refreshNotice && (
              <span
                className={
                  refreshNotice.kind === "error"
                    ? "truncate text-red-300"
                    : "truncate text-emerald-300"
                }
                title={refreshNotice.message}
              >
                {refreshNotice.message}
              </span>
            )}
            {loadedFromCache && (
              <span className="status-chip status-chip-success">
                Cached
              </span>
            )}
          </div>
          <div className="results-metrics">
            <div className="metric metric-selected">
              <div className="metric-label">Selected</div>
              <div className="metric-value truncate">
                {currentNode ? getNodeName(currentNode) : disk}
              </div>
            </div>
            <div className="metric">
              <div className="metric-label">Size</div>
              <div className="metric-value tabular-nums">
                {formatBytes(currentSize)}
              </div>
            </div>
            <div className="metric">
              <div className="metric-label">Items</div>
              <div className="metric-value tabular-nums">
                {currentStats.items.toLocaleString()}
              </div>
            </div>
            <div className="metric">
              <div className="metric-label">Files</div>
              <div className="metric-value tabular-nums">
                {currentStats.files.toLocaleString()}
              </div>
            </div>
            <div className="metric">
              <div className="metric-label">Folders</div>
              <div className="metric-value tabular-nums">
                {currentStats.folders.toLocaleString()}
              </div>
            </div>
          </div>
        </div>
        <div className="results-actions">
          {!isCloud && (
            <button
              type="button"
              onClick={() => setShowScanIssues(true)}
              disabled={!canOpenScanIssues}
              className="button button-secondary"
            >
              <AlertTriangle size={14} />
              Scan Issues
              {issueCount ? ` ${issueCount}` : ""}
            </button>
          )}
          <button
            type="button"
            onClick={startRescan}
            className="button button-secondary"
          >
            <RotateCcw size={14} />
            {isOneDrive || isGoogleDrive || loadedFromCache
              ? "Clean Cache & Rescan"
              : "Rescan"}
          </button>
          {!isCloud && (
            <button
              type="button"
              onClick={() => currentNode && reveal(currentNode)}
              className="button button-secondary"
            >
              <FolderOpen size={14} />
              Reveal
            </button>
          )}
        </div>
      </div>

      <div className="results-split">
        <section className="data-pane">
          <div className="pane-title">
            Tree View
          </div>
          <div
            ref={treeViewportRef}
            onScroll={handleTreeScroll}
            className="min-h-0 flex-1 overflow-auto"
          >
            <table
              className="data-table w-full border-collapse text-xs"
              aria-rowcount={rows.length + (parentNode ? 1 : 0) + 1}
            >
              <thead>
                <tr aria-rowindex={1}>
                  <TableHeader>Name</TableHeader>
                  <TableHeader>Parent %</TableHeader>
                  <TableHeader>Size</TableHeader>
                  {!isCloud && <TableHeader>Allocated</TableHeader>}
                  <TableHeader>Items</TableHeader>
                  <TableHeader>Files</TableHeader>
                  <TableHeader>Folders</TableHeader>
                </tr>
              </thead>
              <tbody>
                {parentNode && (
                  <tr
                    aria-rowindex={2}
                    className="data-row data-row-parent"
                    style={{ height: TREE_ROW_HEIGHT }}
                  >
                    <td className="border-b border-slate-800 px-2 py-1.5 font-medium text-slate-100">
                      <FolderUp className="item-type-icon" size={14} />
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
                    {!isCloud && (
                      <NumberCell>
                        {formatBytes(statsMap.get(parentNode.id)?.size || 0)}
                      </NumberCell>
                    )}
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
                {virtualRange.paddingTop > 0 && (
                  <tr aria-hidden="true">
                    <td
                      colSpan={treeColumnCount}
                      className="border-0 p-0"
                      style={{ height: virtualRange.paddingTop }}
                    />
                  </tr>
                )}
                {virtualRows.map(({ node, depth }, index) => {
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
                      key={
                        node.id ||
                        `${node.name}-${virtualRange.start + index}`
                      }
                      aria-rowindex={
                        virtualRange.start + index + (parentNode ? 3 : 2)
                      }
                      onPointerDown={(event) => startPointerDrag(event, node, deleted)}
                      onContextMenu={(event) => {
                        event.preventDefault();
                        event.stopPropagation();
                        if (deleted) {
                          return;
                        }
                        setContextMenu({
                          node,
                          x: Math.min(event.clientX, window.innerWidth - 224),
                          y: Math.min(event.clientY, window.innerHeight - 210),
                        });
                      }}
                      className={`data-row select-none ${
                        deleted ? "bg-red-950/20 text-red-300" : ""
                      }`}
                      style={{
                        height: TREE_ROW_HEIGHT,
                        ...(deleted
                          ? {
                              textDecoration: "line-through",
                              textDecorationColor: "#f87171",
                              textDecorationThickness: "2px",
                            }
                          : {}),
                      }}
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
                            className="tree-toggle"
                            title={expanded ? "Collapse folder" : "Expand folder"}
                            aria-label={expanded ? "Collapse folder" : "Expand folder"}
                          >
                            {expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                          </button>
                        ) : (
                          <span className="mr-2 inline-block h-4 w-4" />
                        )}
                        {directory ? (
                          <FolderIcon
                            className="item-type-icon"
                            size={14}
                            fill="currentColor"
                          />
                        ) : (
                          <FileIcon className="item-type-icon" size={14} />
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
                      {!isCloud && (
                        <NumberCell>{formatBytes(stats.size)}</NumberCell>
                      )}
                      <NumberCell>{stats.items.toLocaleString()}</NumberCell>
                      <NumberCell>{stats.files.toLocaleString()}</NumberCell>
                      <NumberCell>{stats.folders.toLocaleString()}</NumberCell>
                    </tr>
                  );
                })}
                {virtualRange.paddingBottom > 0 && (
                  <tr aria-hidden="true">
                    <td
                      colSpan={treeColumnCount}
                      className="border-0 p-0"
                      style={{ height: virtualRange.paddingBottom }}
                    />
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </section>

        <section className="data-pane">
          <div className="pane-title">
            File Types
          </div>
          <div className="min-h-0 flex-1 overflow-auto">
            <table className="data-table w-full border-collapse text-xs">
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
                    <tr key={stat.extension} className="data-row">
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

      <div className={`results-bottom ${canDelete ? "" : "results-bottom-full"}`}>
        <div className="treemap-pane">
          <div className="treemap-toolbar">
            <div className="flex min-w-0 items-center gap-2">
              <span className="treemap-title">Storage Map</span>
              <span className="treemap-path">{currentNode?.id || disk}</span>
            </div>
            <span className="treemap-total">
              {rootSize > 0 ? `${formatBytes(rootSize)} scanned` : ""}
            </span>
          </div>
          <div className="treemap-stage">
            {treemapBlocks.map(({ node, index, x0, y0, x1, y1 }) => {
              const blockSize = statsMap.get(node.id)?.size || 0;
              const width = x1 - x0;
              const height = y1 - y0;
              const directory = isDirectory(node);
              const showName = width >= 84 && height >= 25;
              const showSize = width >= 104 && height >= 48;
              return (
                <button
                  key={node.id || `${node.name}-${index}`}
                  type="button"
                  onClick={() => {
                    if (directory) {
                      setCurrentNode(node);
                      setExpandedIds(new Set());
                    } else if (!isCloud) {
                      reveal(node);
                    }
                  }}
                  className="treemap-tile group"
                  style={{
                    left: `${x0 / 10}%`,
                    top: `${(y0 / 108) * 100}%`,
                    width: `${width / 10}%`,
                    height: `${(height / 108) * 100}%`,
                    backgroundColor: treemapColorForIndex(index),
                  }}
                  title={`${getNodeName(node)} - ${formatBytes(blockSize)}`}
                >
                  {showName && (
                    <span className="treemap-tile-name">
                      {directory ? (
                        <FolderIcon size={12} fill="currentColor" />
                      ) : (
                        <FileIcon size={12} />
                      )}
                      <span>{getNodeName(node)}</span>
                    </span>
                  )}
                  {showSize && (
                    <span className="treemap-tile-size">
                      {formatBytes(blockSize)}
                    </span>
                  )}
                </button>
              );
            })}
            {!treemapBlocks.length && (
              <div className="treemap-empty">No sized items in this folder</div>
            )}
          </div>
        </div>
        {canDelete && (
        <div
          ref={dropZoneRef}
          className={`delete-panel ${
            isDeleteTargetActive ? "delete-panel-active" : ""
          }`}
        >
          <div
            className={`delete-dropzone ${
              isDeleteTargetActive
                ? "delete-dropzone-active"
                : ""
            }`}
          >
            {deleteList.length === 0 ? (
              usesTrash
                ? `Drag files or folders here to move them to ${cloudTrashName}`
                : isSsh
                ? "Drag remote files or folders here to permanently delete"
                : "Drag files or folders here to permanently delete"
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
                  {deleteState.error ||
                    (usesTrash ? `Moving to ${cloudTrashName}` : "Deleting")}
                </span>
                <span>
                  {deleteState.current}/{deleteState.total}
                </span>
              </div>
              <div className="delete-progress-track">
                <div
                  className={`delete-progress-fill ${
                    deleteState.error ? "delete-progress-error" : ""
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
              className="button button-secondary flex-1"
            >
              Clear
            </button>
            <button
              onClick={() => {
                if (usesTrash) {
                  deleteSelected();
                } else {
                  setShowPermanentDeleteConfirmation(true);
                }
              }}
              disabled={!deleteList.length || deleteState.isDeleting}
              className="button button-danger flex-1"
            >
              {!deleteState.isDeleting && <Trash2 size={14} />}
              {deleteState.isDeleting
                ? `${usesTrash ? "Moving" : "Deleting"} ${
                    deleteState.current
                  }/${deleteState.total}`
                : usesTrash
                ? "Move to Trash"
                : "Delete Permanently"}
            </button>
          </div>
        </div>
        )}
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
      {contextMenu && (
        <div
          role="menu"
          aria-label={`Actions for ${getNodeName(contextMenu.node)}`}
          onPointerDown={(event) => event.stopPropagation()}
          className="context-menu fixed z-[90] w-56 overflow-hidden py-1 text-xs"
          style={{
            left: Math.max(8, contextMenu.x),
            top: Math.max(8, contextMenu.y),
          }}
        >
          <div className="context-menu-title">
            {getNodeName(contextMenu.node)}
          </div>
          {isDirectory(contextMenu.node) && (
            <button
              role="menuitem"
              onClick={() => {
                setCurrentNode(contextMenu.node);
                setExpandedIds(new Set());
                setContextMenu(null);
              }}
              className="context-menu-item"
            >
              Open Folder
            </button>
          )}
          {canRefreshItem && (
            <button
              role="menuitem"
              disabled={Boolean(refreshingNodeId)}
              onClick={() => refreshNode(contextMenu.node)}
              className="context-menu-item disabled:cursor-not-allowed disabled:opacity-40"
            >
              Refresh This {isDirectory(contextMenu.node) ? "Folder" : "File"}
            </button>
          )}
          {!isCloud && (
            <button
              role="menuitem"
              onClick={() => {
                reveal(contextMenu.node);
                setContextMenu(null);
              }}
              className="context-menu-item"
            >
              Reveal in Finder
            </button>
          )}
          {canDelete && (
            <>
              <div className="context-menu-separator" />
              <button
                role="menuitem"
                onClick={() => {
                  addDeleteTarget(contextMenu.node);
                  setContextMenu(null);
                }}
                className="context-menu-item context-menu-item-danger"
              >
                {usesTrash ? "Add to Trash List" : "Add to Delete List"}
              </button>
            </>
          )}
        </div>
      )}
      {showPermanentDeleteConfirmation && (
        <div className="modal-backdrop">
          <div
            className="app-dialog w-full max-w-lg"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="permanent-delete-title"
          >
            <div className="dialog-header">
              <div>
                <div
                  id="permanent-delete-title"
                  className="flex items-center gap-2 text-sm font-semibold text-white"
                >
                  <AlertTriangle size={16} className="text-red-300" />
                  Permanently delete {deleteList.length} item
                  {deleteList.length === 1 ? "" : "s"}?
                </div>
                <div className="mt-1 text-xs text-slate-400">
                  {isSsh
                    ? "These items will be removed directly from the remote server."
                    : "These items will be removed directly from this Mac."}
                </div>
              </div>
            </div>
            <div className="p-4">
              <div className="max-h-28 overflow-auto rounded border border-[#343b41] bg-[#111519] px-3 py-2 text-xs text-slate-300">
                {deleteList.map((item) => (
                  <div key={item.id} className="truncate py-0.5">
                    {item.id}
                  </div>
                ))}
              </div>
              <p className="mt-3 text-xs font-medium text-red-300">
                This action cannot be undone.
              </p>
              <div className="mt-5 flex justify-end gap-2">
                <button
                  type="button"
                  className="button button-secondary"
                  onClick={() => setShowPermanentDeleteConfirmation(false)}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="button button-danger"
                  onClick={() => {
                    setShowPermanentDeleteConfirmation(false);
                    deleteSelected();
                  }}
                >
                  <Trash2 size={14} />
                  Delete Permanently
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
      {showScanIssues && (
        <div className="modal-backdrop">
          <div className="app-dialog flex max-h-[82vh] w-full max-w-5xl flex-col overflow-hidden">
            <div className="dialog-header">
              <div>
                <div className="text-sm font-semibold text-white">
                  Scan Issues
                </div>
                <div className="mt-1 text-xs text-slate-400">
                  {issueCount
                    ? formatScanIssueCounts(scanIssueReport.counts)
                    : "No scan issues recorded for this result"}
                </div>
              </div>
              <div className="flex items-center gap-2">
                {(hasPermissionIssues(scanIssueReport.counts) || loadedFromCache) && (
                  <button
                    onClick={() => invoke("open_full_disk_access_settings")}
                    className="button button-secondary"
                  >
                    <ShieldCheck size={14} />
                    Grant Full Disk Access
                  </button>
                )}
                <button
                  onClick={() => setShowScanIssues(false)}
                  className="icon-button"
                  title="Close scan issues"
                  aria-label="Close scan issues"
                >
                  <X size={15} />
                </button>
              </div>
            </div>
            <div className="grid grid-cols-4 gap-px border-b border-slate-700 bg-slate-700 text-xs">
              <div className="bg-[#111827] px-3 py-2">
                <div className="text-slate-500">Not permitted</div>
                <div className="mt-1 font-semibold text-slate-100">
                  {scanIssueReport.counts.operationNotPermitted.toLocaleString()}
                </div>
              </div>
              <div className="bg-[#111827] px-3 py-2">
                <div className="text-slate-500">Permission denied</div>
                <div className="mt-1 font-semibold text-slate-100">
                  {scanIssueReport.counts.permissionDenied.toLocaleString()}
                </div>
              </div>
              <div className="bg-[#111827] px-3 py-2">
                <div className="text-slate-500">Interrupted</div>
                <div className="mt-1 font-semibold text-slate-100">
                  {scanIssueReport.counts.interrupted.toLocaleString()}
                </div>
              </div>
              <div className="bg-[#111827] px-3 py-2">
                <div className="text-slate-500">Other</div>
                <div className="mt-1 font-semibold text-slate-100">
                  {scanIssueReport.counts.other.toLocaleString()}
                </div>
              </div>
            </div>
            <div className="min-h-0 flex-1 overflow-auto bg-[#0b1220]">
              {scanIssueReport.records.length ? (
                <table className="w-full border-collapse text-xs">
                  <thead>
                    <tr>
                      <TableHeader>Reason</TableHeader>
                      <TableHeader>Operation</TableHeader>
                      <TableHeader>Path</TableHeader>
                    </tr>
                  </thead>
                  <tbody>
                    {scanIssueReport.records.map((record, index) => (
                      <tr
                        key={`${record.path}-${index}`}
                        className="bg-[#0b1220] hover:bg-[#111827]"
                      >
                        <td className="whitespace-nowrap border-b border-slate-800 px-2 py-1.5 text-slate-200">
                          {record.reason}
                        </td>
                        <td className="whitespace-nowrap border-b border-slate-800 px-2 py-1.5 text-slate-300">
                          {record.operation}
                        </td>
                        <td className="border-b border-slate-800 px-2 py-1.5 text-slate-300">
                          <button
                            onClick={() => reveal({ id: record.path } as DiskItem)}
                            className="max-w-[48rem] truncate text-left hover:underline"
                            title={record.path}
                          >
                            {record.path}
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <div className="p-4 text-sm text-slate-400">
                  {loadedFromCache
                    ? "This result came from cache, so there is no issue report attached. Use Rescan for a fresh full scan."
                    : "No scan issues were recorded."}
                </div>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

export default Scanning;
