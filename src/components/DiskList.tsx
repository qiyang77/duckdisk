import { useEffect, useState } from "react";

import DiskItem from "./DiskItem";
import { invoke } from "@tauri-apps/api/tauri";

import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/api/dialog";
import folderIcon from "../assets/folder.png";
import { useNavigate } from "react-router-dom";

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
      </div>
      <div className="border-t border-slate-700/60 bg-slate-950/60 p-3 text-white w-full flex items-center justify-end gap-4">
        <div className="flex shrink-0 items-center gap-3">
          <button
            onClick={() => invoke("open_full_disk_access_settings")}
            className="rounded border border-sky-500/70 px-3 py-1.5 text-xs font-medium text-sky-100 hover:bg-sky-500/15"
          >
            Grant Full Disk Access
          </button>
          <div className="text-xs text-slate-400">v. {appVersion}</div>
        </div>
      </div>
    </div>
  );
};

export default DiskList;
