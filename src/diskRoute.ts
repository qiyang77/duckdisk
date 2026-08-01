export type DiskRouteState = {
  disk: string;
  used?: number;
  fullscan?: boolean;
  isDirectory?: boolean;
  source?: "local" | "onedrive" | "googledrive" | "ssh";
  accountId?: string;
};

const activeDiskRouteKey = "duckdisk-active-disk-route";

export const rememberDiskRoute = (state: DiskRouteState) => {
  sessionStorage.setItem(activeDiskRouteKey, JSON.stringify(state));
};

export const forgetDiskRoute = () => {
  sessionStorage.removeItem(activeDiskRouteKey);
};

export const readDiskRoute = (): DiskRouteState | null => {
  const saved = sessionStorage.getItem(activeDiskRouteKey);
  if (!saved) {
    return null;
  }

  try {
    const state = JSON.parse(saved) as DiskRouteState;
    return state.disk ? state : null;
  } catch {
    forgetDiskRoute();
    return null;
  }
};
