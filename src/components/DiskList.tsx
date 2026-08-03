import { useEffect, useState } from "react";

import DiskItem from "./DiskItem";
import { invoke } from "@tauri-apps/api/tauri";

import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/api/dialog";
import { open as openExternal } from "@tauri-apps/api/shell";
import folderIcon from "../assets/folder.png";
import oneDriveIcon from "../assets/onedrive.svg";
import googleDriveIcon from "../assets/google-drive.png";
import { useNavigate } from "../router";
import { formatBytes } from "../formatBytes";
import { forgetDiskRoute } from "../diskRoute";
import {
  ChevronRight,
  Cloud,
  Eye,
  EyeOff,
  ExternalLink,
  KeyRound,
  LockKeyhole,
  Plus,
  RefreshCw,
  Server,
  Settings2,
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
  authMethod: "key" | "password";
  storageUsage?: SshStorageUsage | null;
};

type SshStorageUsage = {
  totalSpace: number;
  usedSpace: number;
  availableSpace: number;
};

type SshDraft = Omit<SshConnection, "id"> & {
  id?: string;
  password: string;
};

const emptySshDraft: SshDraft = {
  name: "",
  host: "",
  port: 22,
  path: "/",
  authMethod: "key" as "key" | "password",
  password: "",
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
  const [sshStorageUsage, setSshStorageUsage] = useState<
    Record<string, SshStorageUsage | null>
  >({});
  const [showSshDialog, setShowSshDialog] = useState(false);
  const [editingSshConnection, setEditingSshConnection] =
    useState<SshConnection | null>(null);
  const [showCloudPrivacy, setShowCloudPrivacy] = useState(false);
  const [googleAccountToRevoke, setGoogleAccountToRevoke] =
    useState<GoogleDriveAccount | null>(null);
  const [sshDraft, setSshDraft] = useState(emptySshDraft);
  const [showSshPassword, setShowSshPassword] = useState(false);
  const [cloudBusy, setCloudBusy] = useState<string | null>(null);
  const [oauthCancelRequested, setOauthCancelRequested] = useState<
    "onedrive" | "google" | null
  >(null);
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
    const connections = await invoke<SshConnection[]>("get_ssh_connections");
    setSshConnections(connections);
    setSshStorageUsage(
      Object.fromEntries(
        connections.flatMap((connection) =>
          connection.storageUsage
            ? [[connection.id, connection.storageUsage] as const]
            : []
        )
      )
    );
    void Promise.all(
      connections.map(async (connection) => {
        try {
          const usage = await invoke<SshStorageUsage>("get_ssh_storage_usage", {
            connectionId: connection.id,
          });
          setSshStorageUsage((current) => ({
            ...current,
            [connection.id]: usage,
          }));
        } catch {
          if (!connection.storageUsage) {
            setSshStorageUsage((current) => ({
              ...current,
              [connection.id]: null,
            }));
          }
        }
      })
    );
  };

  useEffect(() => {
    Promise.all([syncOneDrive(), syncGoogleDrive(), syncSshConnections()]).catch(
      (error) => setCloudError(String(error))
    );
  }, []);

  const connectOneDrive = async () => {
    setCloudBusy("connect");
    setOauthCancelRequested(null);
    setCloudError(null);
    try {
      await invoke("connect_onedrive_account");
      await syncOneDrive();
    } catch (error) {
      const message = String(error);
      if (!message.toLowerCase().includes("connection cancelled")) {
        setCloudError(message);
      }
    } finally {
      setOauthCancelRequested(null);
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
    setOauthCancelRequested(null);
    setCloudError(null);
    try {
      await invoke("connect_google_drive_account");
      await syncGoogleDrive();
    } catch (error) {
      const message = String(error);
      if (!message.toLowerCase().includes("connection cancelled")) {
        setCloudError(message);
      }
    } finally {
      setOauthCancelRequested(null);
      setCloudBusy(null);
    }
  };

  const cancelOAuthConnection = async (provider: "onedrive" | "google") => {
    setOauthCancelRequested(provider);
    setCloudError(null);
    try {
      await invoke(
        provider === "onedrive"
          ? "cancel_onedrive_connection"
          : "cancel_google_drive_connection"
      );
    } catch (error) {
      setOauthCancelRequested(null);
      setCloudError(String(error));
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
      setEditingSshConnection(null);
      setSshDraft(emptySshDraft);
      setShowSshPassword(false);
    } catch (error) {
      setCloudError(String(error));
    } finally {
      setCloudBusy(null);
    }
  };

  const openNewSshConnection = () => {
    setEditingSshConnection(null);
    setSshDraft(emptySshDraft);
    setShowSshPassword(false);
    setCloudError(null);
    setShowSshDialog(true);
  };

  const openSshConnectionSettings = (connection: SshConnection) => {
    setEditingSshConnection(connection);
    setSshDraft({ ...connection, password: "" });
    setShowSshPassword(false);
    setCloudError(null);
    setShowSshDialog(true);
  };

  const closeSshDialog = () => {
    setShowSshDialog(false);
    setEditingSshConnection(null);
    setSshDraft(emptySshDraft);
    setShowSshPassword(false);
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
              {cloudBusy === "connect" ? (
                <button
                  type="button"
                  onClick={() => cancelOAuthConnection("onedrive")}
                  disabled={oauthCancelRequested === "onedrive"}
                  className="button button-secondary button-cancel"
                >
                  {oauthCancelRequested === "onedrive" ? (
                    <RefreshCw size={14} className="animate-spin" />
                  ) : (
                    <X size={14} />
                  )}
                  {oauthCancelRequested === "onedrive" ? "Cancelling..." : "Cancel OneDrive"}
                </button>
              ) : (
                <button
                  type="button"
                  onClick={connectOneDrive}
                  disabled={!oneDrive.configured || cloudBusy !== null}
                  className="button button-secondary"
                >
                  <Plus size={14} />
                  OneDrive
                </button>
              )}
              {cloudBusy === "google-connect" ? (
                <button
                  type="button"
                  onClick={() => cancelOAuthConnection("google")}
                  disabled={oauthCancelRequested === "google"}
                  className="button button-secondary button-cancel"
                >
                  {oauthCancelRequested === "google" ? (
                    <RefreshCw size={14} className="animate-spin" />
                  ) : (
                    <X size={14} />
                  )}
                  {oauthCancelRequested === "google" ? "Cancelling..." : "Cancel Google Drive"}
                </button>
              ) : (
                <button
                  type="button"
                  onClick={connectGoogleDrive}
                  disabled={!googleDrive.configured || cloudBusy !== null}
                  className="button button-secondary"
                >
                  <Plus size={14} />
                  Google Drive
                </button>
              )}
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
                          {formatBytes(account.usedSpace)} of {formatBytes(account.totalSpace)} used
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
              <p>Scan a remote path with a saved password or your SSH key configuration</p>
            </div>
            <button type="button" className="button button-secondary" onClick={openNewSshConnection}>
              <Plus size={14} />
              Add SSH Connection
            </button>
          </div>
          <div className="storage-list">
            {sshConnections.length === 0 && (
              <div className="empty-storage-row"><Server size={20} /><span>No SSH connections saved</span></div>
            )}
            {sshConnections.map((connection) => {
              const usage = sshStorageUsage[connection.id];
              return (
                <div
                  key={connection.id}
                  role="button"
                  tabIndex={0}
                  className="storage-row group"
                  onClick={() =>
                    navigate("/disk", {
                      state: {
                        source: "ssh",
                        accountId: connection.id,
                        disk: connection.name,
                        used: usage?.usedSpace ?? 0,
                      },
                    })
                  }
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      navigate("/disk", {
                        state: {
                          source: "ssh",
                          accountId: connection.id,
                          disk: connection.name,
                          used: usage?.usedSpace ?? 0,
                        },
                      });
                    }
                  }}
                >
                  <div className="storage-icon storage-icon-ssh">
                    <Server size={30} />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[14px] font-semibold text-[#f4f6f7]">
                      {connection.name}
                    </div>
                    <div className="mt-1 truncate text-[12px] tabular-nums text-[#a6afb8]">
                      {usage === undefined
                        ? "Checking storage usage..."
                        : usage === null
                        ? "Storage usage unavailable"
                        : `${formatBytes(usage.usedSpace)} of ${formatBytes(
                            usage.totalSpace
                          )} used`}
                    </div>
                  </div>
                  <button
                    type="button"
                    className="icon-button"
                    title={`Edit ${connection.name}`}
                    aria-label={`Edit ${connection.name}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      openSshConnectionSettings(connection);
                    }}
                    disabled={cloudBusy !== null}
                  >
                    <Settings2 size={14} />
                  </button>
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
                    {cloudBusy === `ssh-${connection.id}` ? (
                      <RefreshCw size={14} className="animate-spin" />
                    ) : (
                      <Unplug size={14} />
                    )}
                  </button>
                  <ChevronRight className="storage-chevron" size={17} />
                </div>
              );
            })}
          </div>
        </section>
      </div>
      {showSshDialog && (
        <div className="modal-backdrop">
          <div className="app-dialog ssh-dialog">
            <div className="dialog-header">
              <div>
                <div className="text-sm font-semibold text-white">
                  {editingSshConnection ? "SSH Connection Settings" : "Add SSH Connection"}
                </div>
                <div className="mt-1 text-xs text-slate-400">
                  {editingSshConnection
                    ? "Update this server's address, path, or authentication"
                    : "Choose how DuckDisk should authenticate with this server"}
                </div>
              </div>
              <button className="icon-button" title="Close" onClick={closeSshDialog}><X size={15} /></button>
            </div>
            <div className="ssh-form">
              <label><span>Name</span><input value={sshDraft.name} placeholder="Production server" onChange={(event) => setSshDraft({ ...sshDraft, name: event.target.value })} /></label>
              <label><span>Host or SSH alias</span><input value={sshDraft.host} placeholder="user@example.com or my-server" onChange={(event) => setSshDraft({ ...sshDraft, host: event.target.value })} /></label>
              <div className="ssh-form-grid">
                <label><span>Port</span><input type="number" min="1" max="65535" value={sshDraft.port} onChange={(event) => setSshDraft({ ...sshDraft, port: Number(event.target.value) })} /></label>
                <label><span>Remote path</span><input value={sshDraft.path} placeholder="/" onChange={(event) => setSshDraft({ ...sshDraft, path: event.target.value })} /></label>
              </div>
              <div className="ssh-auth-method" role="group" aria-label="SSH authentication method">
                <button
                  type="button"
                  className={sshDraft.authMethod === "password" ? "active" : ""}
                  onClick={() => setSshDraft({ ...sshDraft, authMethod: "password" })}
                >
                  <LockKeyhole size={14} />
                  Password
                </button>
                <button
                  type="button"
                  className={sshDraft.authMethod === "key" ? "active" : ""}
                  onClick={() => setSshDraft({ ...sshDraft, authMethod: "key", password: "" })}
                >
                  <KeyRound size={14} />
                  I have configured an SSH key
                </button>
              </div>
              {sshDraft.authMethod === "password" && (
                <label>
                  <span>Password</span>
                  <div className="ssh-password-field">
                    <input
                      type={showSshPassword ? "text" : "password"}
                      value={sshDraft.password}
                      autoComplete="off"
                      placeholder={
                        editingSshConnection?.authMethod === "password"
                          ? "Leave blank to keep the current password"
                          : "SSH account password"
                      }
                      onChange={(event) => setSshDraft({ ...sshDraft, password: event.target.value })}
                    />
                    <button
                      type="button"
                      className="icon-button"
                      title={showSshPassword ? "Hide password" : "Show password"}
                      aria-label={showSshPassword ? "Hide password" : "Show password"}
                      onClick={() => setShowSshPassword((visible) => !visible)}
                    >
                      {showSshPassword ? <EyeOff size={14} /> : <Eye size={14} />}
                    </button>
                  </div>
                </label>
              )}
              <div className="ssh-form-note">
                {sshDraft.authMethod === "password"
                  ? editingSshConnection?.authMethod === "password"
                    ? "Leave the password blank to keep it unchanged. Passwords are stored in macOS Keychain."
                    : "The password is stored in macOS Keychain and is never written to DuckDisk's connection file."
                  : "DuckDisk uses macOS ssh, ~/.ssh/config, SSH Agent, and your existing private keys."}
              </div>
              {cloudError && (
                <div className="inline-alert inline-alert-error">
                  {cloudError}
                </div>
              )}
              <div className="ssh-form-actions">
                <button className="button button-secondary" onClick={closeSshDialog}>Cancel</button>
                <button
                  className="button button-primary"
                  disabled={
                    !sshDraft.host.trim()
                    || (sshDraft.authMethod === "password"
                      && !sshDraft.password
                      && editingSshConnection?.authMethod !== "password")
                    || cloudBusy !== null
                  }
                  onClick={saveSshConnection}
                >
                  {cloudBusy === "ssh-save" && <RefreshCw size={14} className="animate-spin" />}
                  {editingSshConnection ? "Save Changes" : "Save Connection"}
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
                    Revoke Google Drive access for {googleAccountToRevoke.name}?
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
