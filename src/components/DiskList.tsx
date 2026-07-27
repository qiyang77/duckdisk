import { useEffect, useState } from "react";

import DiskItem from "./DiskItem";
import { invoke } from "@tauri-apps/api/tauri";

import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/api/dialog";
import { open as openExternal } from "@tauri-apps/api/shell";
import folderIcon from "../assets/folder.png";
import { useNavigate } from "react-router-dom";
import { formatBytes } from "../formatBytes";

type OneDriveAccount = {
  id: string;
  name: string;
  driveType: string;
  totalSpace: number;
  usedSpace: number;
  availableSpace: number;
};

type OneDriveState = {
  configured: boolean;
  accounts: OneDriveAccount[];
};

type UpdateCheck = {
  currentVersion: string;
  latestVersion: string;
  updateAvailable: boolean;
  releaseUrl: string;
};

const CloudIcon = () => (
  <svg viewBox="0 0 64 64" aria-hidden="true" className="h-14 w-14">
    <path
      d="M22 48h28a11 11 0 0 0 1-21.9A17 17 0 0 0 19.3 21 13.5 13.5 0 0 0 22 48Z"
      fill="#38bdf8"
    />
    <path
      d="M8 48h30a10 10 0 0 0 0-20h-1.2A15 15 0 0 0 8.4 31.6 8.5 8.5 0 0 0 8 48Z"
      fill="#0284c7"
    />
  </svg>
);

declare global {
  interface Window {
    electron: any;
    analytics: any;
    configStore: any;
    licver: any;
  }
}

const DiskList = () => {
  const [disks, setDisks] = useState([]);
  const [appVersion, setAppVersion] = useState("1.0.0");
  const [oneDrive, setOneDrive] = useState<OneDriveState>({
    configured: false,
    accounts: [],
  });
  const [cloudBusy, setCloudBusy] = useState<string | null>(null);
  const [cloudError, setCloudError] = useState<string | null>(null);
  const [isCheckingUpdates, setCheckingUpdates] = useState(false);
  const [updateCheck, setUpdateCheck] = useState<UpdateCheck | null>(null);
  const [updateError, setUpdateError] = useState<string | null>(null);
  const navigate = useNavigate();
  useEffect(() => {
    getVersion().then((v) => setAppVersion(v));
    //   window.electron.app
    // setAppVersion(window.electron.appInfo().version)
  }, []);

  useEffect(() => {
    // window.electron.diskUtils.killDiskSizeWorker();
    const syncDisks = async () => {
      const disksString: string = await invoke("get_disks");
      const disks = JSON.parse(disksString);
      setDisks(
        disks.filter((disk: any) => disk.sMountPoint !== "/System/Volumes/Data")
      );
    };
    const handle = setInterval(syncDisks, 2000);
    syncDisks();
    return () => {
      clearInterval(handle);
    };
  }, []);

  const syncOneDrive = async () => {
    const state = await invoke<OneDriveState>("get_onedrive_state");
    setOneDrive(state);
  };

  useEffect(() => {
    syncOneDrive().catch((error) => setCloudError(String(error)));
  }, []);

  const connectOneDrive = async () => {
    setCloudBusy("connect");
    setCloudError(null);
    try {
      await invoke("connect_onedrive_account");
      await syncOneDrive();
    } catch (error) {
      setCloudError(String(error));
    } finally {
      setCloudBusy(null);
    }
  };

  const disconnectOneDrive = async (account: OneDriveAccount) => {
    setCloudBusy(account.id);
    setCloudError(null);
    try {
      await invoke("disconnect_onedrive_account", { accountId: account.id });
      await syncOneDrive();
    } catch (error) {
      setCloudError(String(error));
    } finally {
      setCloudBusy(null);
    }
  };

  const checkForUpdates = async () => {
    setCheckingUpdates(true);
    setUpdateCheck(null);
    setUpdateError(null);
    try {
      setUpdateCheck(await invoke<UpdateCheck>("check_for_updates"));
    } catch (error) {
      setUpdateError(String(error));
    } finally {
      setCheckingUpdates(false);
    }
  };

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      <div className="text-white flex-1 overflow-auto">
        {disks.map((disk: any) => (
          <DiskItem key={disk.sMountPoint} disk={disk}></DiskItem>
        ))}
        <div
          className="text-white p-4 flex gap-4 items-center hover:bg-gray-800 cursor-pointer"
          onClick={() => {
            open({
              multiple: false,
              directory: true,
            }).then((directory) => {
              if (directory)
                navigate("/disk", {
                  state: {
                    disk: (directory as string).replace(/\\/g, "/"),
                    used: 0,
                    fullscan: true,
                    isDirectory: true,
                  },
                });
              console.log({ directory });
            });
          }}
        >
          <div className="w-16 h-16 flex justify-center items-center align-middle">
            <img src={folderIcon} className="w-12 h-12 opacity-70"></img>
          </div>
          <div className="flex-1">
            <div className="flex justify-between mb-1">
              <span className="font-medium  text-white text-sm">
                Select a folder to Scan
                {/* <span className="opacity-60"></span> */}
              </span>
            </div>
          </div>
        </div>
        <section className="border-t border-slate-700/70 bg-slate-950/30">
          <div className="flex items-center justify-between px-4 py-3">
            <div>
              <div className="text-xs font-semibold uppercase text-slate-400">
                Cloud Storage
              </div>
              <div className="mt-0.5 text-xs text-slate-500">
                Scan cloud metadata without downloading file contents
              </div>
            </div>
            <button
              onClick={connectOneDrive}
              disabled={!oneDrive.configured || cloudBusy !== null}
              className="rounded border border-sky-500/70 px-3 py-1.5 text-xs font-medium text-sky-100 hover:bg-sky-500/15 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {cloudBusy === "connect" ? "Waiting for Microsoft..." : "Connect OneDrive"}
            </button>
          </div>
          {!oneDrive.configured && (
            <div className="border-t border-amber-900/50 bg-amber-950/20 px-4 py-2 text-xs text-amber-200">
              OneDrive sign-in is not configured in this build.
            </div>
          )}
          {cloudError && (
            <div className="border-t border-red-900/50 bg-red-950/20 px-4 py-2 text-xs text-red-300">
              {cloudError}
            </div>
          )}
          {oneDrive.accounts.map((account) => {
            const percent =
              account.totalSpace > 0
                ? Math.min(100, (account.usedSpace / account.totalSpace) * 100)
                : 0;
            return (
              <div
                key={account.id}
                onClick={() =>
                  navigate("/disk", {
                    state: {
                      source: "onedrive",
                      accountId: account.id,
                      disk: `OneDrive - ${account.name}`,
                      used: account.usedSpace,
                    },
                  })
                }
                className="flex cursor-pointer items-center gap-4 border-t border-slate-800 px-4 py-3 hover:bg-slate-800"
              >
                <div className="flex h-16 w-16 shrink-0 items-center justify-center">
                  <CloudIcon />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="flex items-start justify-between gap-4">
                    <div className="min-w-0">
                      <div className="truncate text-sm font-semibold text-white">
                        OneDrive - {account.name}
                      </div>
                      <div className="mt-1 text-xs text-slate-400">
                        {formatBytes(account.usedSpace)} of{" "}
                        {formatBytes(account.totalSpace)} used
                      </div>
                    </div>
                    <div className="shrink-0 text-right text-xs text-slate-300">
                      {percent.toFixed(0)}%
                      <div className="mt-0.5 text-slate-500">
                        {formatBytes(account.availableSpace)} free
                      </div>
                    </div>
                  </div>
                  <div className="mt-2 h-2 overflow-hidden rounded-full bg-slate-700">
                    <div
                      className="h-full rounded-full bg-sky-500"
                      style={{ width: `${percent}%` }}
                    />
                  </div>
                </div>
                <button
                  onClick={(event) => {
                    event.stopPropagation();
                    disconnectOneDrive(account);
                  }}
                  disabled={cloudBusy !== null}
                  className="shrink-0 rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 hover:border-red-500/70 hover:text-red-300 disabled:opacity-40"
                >
                  {cloudBusy === account.id ? "Disconnecting..." : "Disconnect"}
                </button>
              </div>
            );
          })}
        </section>
      </div>
      <div className="border-t border-slate-700/60 bg-slate-950/60 p-3 text-white w-full flex items-center justify-end gap-4">
        <div className="flex shrink-0 items-center gap-3">
          {updateCheck && (
            <span
              className={`text-xs ${
                updateCheck.updateAvailable
                  ? "text-amber-300"
                  : "text-emerald-300"
              }`}
            >
              {updateCheck.updateAvailable
                ? `v ${updateCheck.latestVersion} available`
                : "DuckDisk is up to date"}
            </span>
          )}
          {updateError && (
            <span className="max-w-[260px] truncate text-xs text-red-300">
              {updateError}
            </span>
          )}
          {updateCheck?.updateAvailable && (
            <button
              onClick={() => openExternal(updateCheck.releaseUrl)}
              className="rounded border border-amber-500/70 px-3 py-1.5 text-xs font-medium text-amber-100 hover:bg-amber-500/15"
            >
              View Release
            </button>
          )}
          <button
            onClick={checkForUpdates}
            disabled={isCheckingUpdates}
            className="rounded border border-slate-600 px-3 py-1.5 text-xs font-medium text-slate-100 hover:bg-slate-800 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {isCheckingUpdates ? "Checking..." : "Check for Updates"}
          </button>
          <button
            onClick={() => invoke("open_full_disk_access_settings")}
            className="rounded border border-sky-500/70 px-3 py-1.5 text-xs font-medium text-sky-100 hover:bg-sky-500/15"
          >
            Grant Full Disk Access
          </button>
          <div className="text-xs text-slate-400">v {appVersion}</div>
        </div>
      </div>
    </div>
  );
};

export default DiskList;
