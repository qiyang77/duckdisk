import { useEffect, useState } from "react";

import DiskItem from "./DiskItem";
import { invoke } from "@tauri-apps/api/tauri";

import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/api/dialog";
import { open as openExternal } from "@tauri-apps/api/shell";
import folderIcon from "../assets/folder.png";
import oneDriveIcon from "../assets/onedrive.svg";
import googleDriveIcon from "../assets/google-drive.png";
import { useNavigate } from "react-router-dom";
import { formatBytes } from "../formatBytes";
import { forgetDiskRoute } from "../diskRoute";
import {
  ChevronRight,
  Cloud,
  ExternalLink,
  Plus,
  RefreshCw,
  Server,
  ShieldCheck,
  Unplug,
  X,
} from "lucide-react";

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

type GoogleDriveAccount = {
  id: string;
  name: string;
  email: string;
  totalSpace: number;
  usedSpace: number;
  availableSpace: number;
};

type GoogleDriveState = {
  configured: boolean;
  accounts: GoogleDriveAccount[];
};

type SshConnection = {
  id: string;
  name: string;
  host: string;
  port: number;
  path: string;
};

type UpdateCheck = {
  currentVersion: string;
  latestVersion: string;
  updateAvailable: boolean;
  releaseUrl: string;
};

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
  const [googleDrive, setGoogleDrive] = useState<GoogleDriveState>({
    configured: false,
    accounts: [],
  });
  const [sshConnections, setSshConnections] = useState<SshConnection[]>([]);
  const [showSshDialog, setShowSshDialog] = useState(false);
  const [showCloudPrivacy, setShowCloudPrivacy] = useState(false);
  const [googleAccountToRevoke, setGoogleAccountToRevoke] =
    useState<GoogleDriveAccount | null>(null);
  const [sshDraft, setSshDraft] = useState({
    name: "",
    host: "",
    port: 22,
    path: "/",
  });
  const [cloudBusy, setCloudBusy] = useState<string | null>(null);
  const [cloudError, setCloudError] = useState<string | null>(null);
  const [isCheckingUpdates, setCheckingUpdates] = useState(false);
  const [updateCheck, setUpdateCheck] = useState<UpdateCheck | null>(null);
  const [updateError, setUpdateError] = useState<string | null>(null);
  const navigate = useNavigate();
  useEffect(() => {
    forgetDiskRoute();
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

  const syncGoogleDrive = async () => {
    const state = await invoke<GoogleDriveState>("get_google_drive_state");
    setGoogleDrive(state);
  };

  const syncSshConnections = async () => {
    setSshConnections(await invoke<SshConnection[]>("get_ssh_connections"));
  };

  useEffect(() => {
    Promise.all([syncOneDrive(), syncGoogleDrive(), syncSshConnections()]).catch(
      (error) => setCloudError(String(error))
    );
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

  const connectGoogleDrive = async () => {
    setCloudBusy("google-connect");
    setCloudError(null);
    try {
      await invoke("connect_google_drive_account");
      await syncGoogleDrive();
    } catch (error) {
      setCloudError(String(error));
    } finally {
      setCloudBusy(null);
    }
  };

  const revokeGoogleDrive = async (account: GoogleDriveAccount) => {
    setCloudBusy(`google-${account.id}`);
    setCloudError(null);
    try {
      await invoke("revoke_google_drive_account", { accountId: account.id });
      await syncGoogleDrive();
      setGoogleAccountToRevoke(null);
      setShowCloudPrivacy(false);
    } catch (error) {
      setCloudError(String(error));
    } finally {
      setCloudBusy(null);
    }
  };

  const saveSshConnection = async () => {
    setCloudBusy("ssh-save");
    setCloudError(null);
    try {
      await invoke("save_ssh_connection", { connection: sshDraft });
      await syncSshConnections();
      setShowSshDialog(false);
      setSshDraft({ name: "", host: "", port: 22, path: "/" });
    } catch (error) {
      setCloudError(String(error));
    } finally {
      setCloudBusy(null);
    }
  };

  const removeSshConnection = async (connection: SshConnection) => {
    setCloudBusy(`ssh-${connection.id}`);
    setCloudError(null);
    try {
      await invoke("remove_ssh_connection", { connectionId: connection.id });
      await syncSshConnections();
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
    <div className="storage-overview">
      <div className="storage-scroll">
        <section className="storage-section" aria-labelledby="local-storage-title">
          <div className="section-toolbar">
            <div>
              <h1 id="local-storage-title">Local Storage</h1>
              <p>{disks.length} {disks.length === 1 ? "volume" : "volumes"} available</p>
            </div>
          </div>
          <div className="storage-list">
            {disks.map((disk: any) => (
              <DiskItem key={disk.sMountPoint} disk={disk}></DiskItem>
            ))}
            <button
              type="button"
              className="storage-row group w-full text-left"
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
                });
              }}
            >
              <div className="storage-icon storage-icon-folder">
                <img src={folderIcon} alt="" />
              </div>
              <div className="min-w-0 flex-1">
                <div className="text-[14px] font-semibold text-[#f4f6f7]">
                  Scan a Folder
                </div>
                <div className="mt-1 text-[12px] text-[#7f8993]">
                  Choose a specific folder instead of an entire disk
                </div>
              </div>
              <ChevronRight className="storage-chevron" size={17} />
            </button>
          </div>
        </section>

        <section className="storage-section" aria-labelledby="cloud-storage-title">
          <div className="section-toolbar">
            <div>
              <h2 id="cloud-storage-title">Cloud Storage</h2>
              <p>OneDrive and Google Drive metadata scanning</p>
            </div>
            <div className="section-actions">
              <button
                type="button"
                onClick={() => setShowCloudPrivacy(true)}
                className="button button-secondary"
              >
                <ShieldCheck size={14} />
                Privacy & Access
              </button>
              <button
                type="button"
                onClick={connectOneDrive}
                disabled={!oneDrive.configured || cloudBusy !== null}
                className="button button-primary"
              >
                {cloudBusy === "connect" ? <RefreshCw size={14} className="animate-spin" /> : <Plus size={14} />}
                OneDrive
              </button>
              <button
                type="button"
                onClick={connectGoogleDrive}
                disabled={!googleDrive.configured || cloudBusy !== null}
                className="button button-secondary"
              >
                {cloudBusy === "google-connect" ? <RefreshCw size={14} className="animate-spin" /> : <Plus size={14} />}
                Google Drive
              </button>
            </div>
          </div>
          {!oneDrive.configured && (
            <div className="inline-alert inline-alert-warning">
              OneDrive sign-in is not configured in this build.
            </div>
          )}
          {!googleDrive.configured && (
            <div className="inline-alert inline-alert-warning">
              Google Drive sign-in is not configured in this build.
            </div>
          )}
          {cloudError && (
            <div className="inline-alert inline-alert-error">
              {cloudError}
            </div>
          )}
          <div className="storage-list">
            {oneDrive.accounts.length === 0 && oneDrive.configured && (
              <div className="empty-storage-row">
                <Cloud size={20} />
                <span>No OneDrive accounts connected</span>
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
                  role="button"
                  tabIndex={0}
                  aria-label={`Scan OneDrive - ${account.name}`}
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
                  onKeyDown={(event) => {
                    if (
                      event.target === event.currentTarget &&
                      (event.key === "Enter" || event.key === " ")
                    ) {
                      event.preventDefault();
                      navigate("/disk", {
                        state: {
                          source: "onedrive",
                          accountId: account.id,
                          disk: `OneDrive - ${account.name}`,
                          used: account.usedSpace,
                        },
                      });
                    }
                  }}
                  className="storage-row group"
                >
                  <div className="storage-icon storage-icon-onedrive">
                    <img src={oneDriveIcon} alt="" />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-start justify-between gap-6">
                      <div className="min-w-0">
                        <div className="truncate text-[14px] font-semibold text-[#f4f6f7]">
                          OneDrive - {account.name}
                        </div>
                        <div className="mt-1 text-[12px] tabular-nums text-[#a6afb8]">
                          {formatBytes(account.usedSpace)} of{" "}
                          {formatBytes(account.totalSpace)} used
                        </div>
                      </div>
                      <div className="flex shrink-0 items-center gap-3">
                        <div className="text-right tabular-nums">
                          <div className="text-[13px] font-semibold text-[#eef1f3]">
                            {percent.toFixed(0)}%
                          </div>
                          <div className="mt-0.5 text-[11px] text-[#7f8993]">
                            {formatBytes(account.availableSpace)} free
                          </div>
                        </div>
                        <button
                          type="button"
                          title={`Disconnect OneDrive - ${account.name}`}
                          aria-label={`Disconnect OneDrive - ${account.name}`}
                          onClick={(event) => {
                            event.stopPropagation();
                            disconnectOneDrive(account);
                          }}
                          disabled={cloudBusy !== null}
                          className="icon-button icon-button-danger"
                        >
                          {cloudBusy === account.id ? (
                            <RefreshCw size={14} className="animate-spin" />
                          ) : (
                            <Unplug size={14} />
                          )}
                        </button>
                        <ChevronRight className="storage-chevron" size={17} />
                      </div>
                    </div>
                    <div className="storage-meter" data-tone="cloud">
                      <div style={{ width: `${percent}%` }} />
                    </div>
                  </div>
                </div>
              );
            })}
            {googleDrive.accounts.map((account) => {
              const percent = account.totalSpace > 0
                ? Math.min(100, (account.usedSpace / account.totalSpace) * 100)
                : 0;
              return (
                <div
                  key={`google-${account.id}`}
                  role="button"
                  tabIndex={0}
                  className="storage-row group"
                  onClick={() => navigate("/disk", { state: {
                    source: "googledrive",
                    accountId: account.id,
                    disk: `Google Drive - ${account.name}`,
                    used: account.usedSpace,
                  }})}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      navigate("/disk", { state: {
                        source: "googledrive",
                        accountId: account.id,
                        disk: `Google Drive - ${account.name}`,
                        used: account.usedSpace,
                      }});
                    }
                  }}
                >
                  <div className="storage-icon storage-icon-google-drive">
                    <img src={googleDriveIcon} alt="" />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-start justify-between gap-6">
                      <div className="min-w-0">
                        <div className="truncate text-[14px] font-semibold text-[#f4f6f7]">
                          Google Drive - {account.name}
                        </div>
                        <div className="mt-1 truncate text-[12px] tabular-nums text-[#a6afb8]">
                          {formatBytes(account.usedSpace)} of {formatBytes(account.totalSpace)} used · {account.email}
                        </div>
                      </div>
                      <div className="flex shrink-0 items-center gap-3">
                        <div className="text-right tabular-nums">
                          <div className="text-[13px] font-semibold text-[#eef1f3]">{percent.toFixed(0)}%</div>
                          <div className="mt-0.5 text-[11px] text-[#7f8993]">{formatBytes(account.availableSpace)} free</div>
                        </div>
                        <button
                          type="button"
                          className="icon-button icon-button-danger"
                          title={`Revoke Google Drive access - ${account.name}`}
                          aria-label={`Revoke Google Drive access - ${account.name}`}
                          onClick={(event) => {
                            event.stopPropagation();
                            setGoogleAccountToRevoke(account);
                            setShowCloudPrivacy(true);
                          }}
                          disabled={cloudBusy !== null}
                        >
                          {cloudBusy === `google-${account.id}` ? <RefreshCw size={14} className="animate-spin" /> : <Unplug size={14} />}
                        </button>
                        <ChevronRight className="storage-chevron" size={17} />
                      </div>
                    </div>
                    <div className="storage-meter" data-tone="google"><div style={{ width: `${percent}%` }} /></div>
                  </div>
                </div>
              );
            })}
          </div>
        </section>

        <section className="storage-section" aria-labelledby="remote-storage-title">
          <div className="section-toolbar">
            <div>
              <h2 id="remote-storage-title">Remote Servers</h2>
              <p>Scan a remote path through your existing SSH keys and config</p>
            </div>
            <button type="button" className="button button-secondary" onClick={() => setShowSshDialog(true)}>
              <Plus size={14} />
              Add SSH Connection
            </button>
          </div>
          <div className="storage-list">
            {sshConnections.length === 0 && (
              <div className="empty-storage-row"><Server size={20} /><span>No SSH connections saved</span></div>
            )}
            {sshConnections.map((connection) => (
              <div
                key={connection.id}
                role="button"
                tabIndex={0}
                className="storage-row group"
                onClick={() => navigate("/disk", { state: {
                  source: "ssh",
                  accountId: connection.id,
                  disk: connection.name,
                  used: 0,
                }})}
                onKeyDown={(event) => {
                  if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    navigate("/disk", { state: {
                      source: "ssh",
                      accountId: connection.id,
                      disk: connection.name,
                      used: 0,
                    }});
                  }
                }}
              >
                <div className="storage-icon storage-icon-ssh"><Server size={30} /></div>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[14px] font-semibold text-[#f4f6f7]">{connection.name}</div>
                  <div className="mt-1 truncate text-[12px] text-[#a6afb8]">{connection.host}:{connection.port} · {connection.path}</div>
                </div>
                <button
                  type="button"
                  className="icon-button icon-button-danger"
                  title={`Remove ${connection.name}`}
                  onClick={(event) => {
                    event.stopPropagation();
                    removeSshConnection(connection);
                  }}
                  disabled={cloudBusy !== null}
                >
                  {cloudBusy === `ssh-${connection.id}` ? <RefreshCw size={14} className="animate-spin" /> : <Unplug size={14} />}
                </button>
                <ChevronRight className="storage-chevron" size={17} />
              </div>
            ))}
          </div>
        </section>
      </div>
      {showSshDialog && (
        <div className="modal-backdrop">
          <div className="app-dialog ssh-dialog">
            <div className="dialog-header">
              <div>
                <div className="text-sm font-semibold text-white">Add SSH Connection</div>
                <div className="mt-1 text-xs text-slate-400">Uses macOS ssh, ~/.ssh/config, and your existing keys</div>
              </div>
              <button className="icon-button" title="Close" onClick={() => setShowSshDialog(false)}><X size={15} /></button>
            </div>
            <div className="ssh-form">
              <label><span>Name</span><input value={sshDraft.name} placeholder="Production server" onChange={(event) => setSshDraft({ ...sshDraft, name: event.target.value })} /></label>
              <label><span>Host or SSH alias</span><input value={sshDraft.host} placeholder="user@example.com or my-server" onChange={(event) => setSshDraft({ ...sshDraft, host: event.target.value })} /></label>
              <div className="ssh-form-grid">
                <label><span>Port</span><input type="number" min="1" max="65535" value={sshDraft.port} onChange={(event) => setSshDraft({ ...sshDraft, port: Number(event.target.value) })} /></label>
                <label><span>Remote path</span><input value={sshDraft.path} placeholder="/" onChange={(event) => setSshDraft({ ...sshDraft, path: event.target.value })} /></label>
              </div>
              <div className="ssh-form-note">Password prompts are not stored or shown. Configure key-based login in Terminal before scanning.</div>
              <div className="ssh-form-actions">
                <button className="button button-secondary" onClick={() => setShowSshDialog(false)}>Cancel</button>
                <button className="button button-primary" disabled={!sshDraft.host.trim() || cloudBusy !== null} onClick={saveSshConnection}>
                  {cloudBusy === "ssh-save" && <RefreshCw size={14} className="animate-spin" />}
                  Save Connection
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
      {showCloudPrivacy && (
        <div className="modal-backdrop">
          <div
            className="app-dialog w-full max-w-2xl"
            role="dialog"
            aria-modal="true"
            aria-labelledby="cloud-privacy-title"
          >
            <div className="dialog-header">
              <div>
                <div id="cloud-privacy-title" className="text-sm font-semibold text-white">
                  Cloud Privacy & Account Access
                </div>
                <div className="mt-1 text-xs text-slate-400">
                  What DuckDisk reads, stores, and can change
                </div>
              </div>
              <button
                className="icon-button"
                title="Close"
                onClick={() => {
                  setShowCloudPrivacy(false);
                  setGoogleAccountToRevoke(null);
                }}
              >
                <X size={15} />
              </button>
            </div>
            <div className="space-y-4 p-5 text-sm text-slate-300">
              <p>
                DuckDisk reads file and folder metadata to calculate storage use.
                It does not download cloud file contents for analysis.
              </p>
              <p>
                Sign-in tokens are stored in macOS Keychain and scan metadata is
                cached only on this Mac. DuckDisk does not operate an account-data server.
              </p>
              <p>
                Deleting from a cloud scan moves the selected item to OneDrive's
                Recycle Bin or Google Drive Trash, where the provider may allow recovery.
              </p>
              <div className="flex flex-wrap gap-3">
                <button
                  type="button"
                  className="button button-secondary"
                  onClick={() => openExternal("https://duckdisk.com/privacy.html")}
                >
                  <ExternalLink size={14} />
                  Privacy Policy
                </button>
                <button
                  type="button"
                  className="button button-secondary"
                  onClick={() => openExternal("https://duckdisk.com/terms.html")}
                >
                  <ExternalLink size={14} />
                  Terms
                </button>
              </div>
              {googleAccountToRevoke && (
                <div className="rounded border border-red-400/30 bg-red-950/20 p-4">
                  <div className="font-semibold text-red-100">
                    Revoke Google Drive access for {googleAccountToRevoke.email}?
                  </div>
                  <p className="mt-2 text-xs leading-5 text-red-200/80">
                    This invalidates DuckDisk's Google token and removes the saved
                    account and scan cache from this Mac. Your Drive files are not deleted.
                  </p>
                  <div className="mt-4 flex justify-end gap-2">
                    <button
                      type="button"
                      className="button button-secondary"
                      onClick={() => setGoogleAccountToRevoke(null)}
                      disabled={cloudBusy !== null}
                    >
                      Cancel
                    </button>
                    <button
                      type="button"
                      className="button button-danger"
                      onClick={() => revokeGoogleDrive(googleAccountToRevoke)}
                      disabled={cloudBusy !== null}
                    >
                      {cloudBusy === `google-${googleAccountToRevoke.id}` ? (
                        <RefreshCw size={14} className="animate-spin" />
                      ) : (
                        <Unplug size={14} />
                      )}
                      Revoke Access
                    </button>
                  </div>
                </div>
              )}
            </div>
          </div>
        </div>
      )}
      <footer className="app-footer">
        <div className="flex min-w-0 items-center gap-3">
          {updateCheck && (
            <span
              className={`status-text ${
                updateCheck.updateAvailable
                  ? "status-text-warning"
                  : "status-text-success"
              }`}
            >
              {updateCheck.updateAvailable
                ? `v ${updateCheck.latestVersion} available`
                : "DuckDisk is up to date"}
            </span>
          )}
          {updateError && (
            <span className="status-text status-text-error max-w-[260px] truncate">
              {updateError}
            </span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {updateCheck?.updateAvailable && (
            <button
              type="button"
              onClick={() => openExternal(updateCheck.releaseUrl)}
              className="button button-warning"
            >
              <ExternalLink size={14} />
              View Release
            </button>
          )}
          <button
            type="button"
            onClick={checkForUpdates}
            disabled={isCheckingUpdates}
            className="button button-secondary"
          >
            <RefreshCw size={14} className={isCheckingUpdates ? "animate-spin" : ""} />
            {isCheckingUpdates ? "Checking..." : "Check for Updates"}
          </button>
          <button
            type="button"
            onClick={() => invoke("open_full_disk_access_settings")}
            className="button button-secondary"
          >
            <ShieldCheck size={14} />
            Grant Full Disk Access
          </button>
          <div className="version-label">v {appVersion}</div>
        </div>
      </footer>
    </div>
  );
};

export default DiskList;
