import diskIcon from "../assets/harddisk.png";
import removableDriver from "../assets/removable-drive.png";

import { useNavigate } from "../router";
import { formatBytes } from "../formatBytes";
import { ChevronRight } from "lucide-react";
import { open } from "@tauri-apps/api/dialog";

const isMacAppStore = import.meta.env.VITE_DISTRIBUTION === "mas";

const DiskItem = ({ disk }: any) => {
  const navigate = useNavigate();
  const usedSpace = Math.max(0, disk.totalSpace - disk.availableSpace);
  const perc = disk.totalSpace > 0 ? usedSpace / disk.totalSpace : 0;
  const usageTone = perc >= 0.85 ? "critical" : perc >= 0.7 ? "warning" : "healthy";

  const icona = disk.isRemovable ? removableDriver : diskIcon;
  const scanDisk = async () => {
    let scanPath = disk.sMountPoint;
    if (isMacAppStore) {
      const selected = await open({
        multiple: false,
        directory: true,
        defaultPath: disk.sMountPoint,
        title: `Allow DuckDisk to scan ${disk.name || "this volume"}`,
      });
      if (typeof selected !== "string") return;
      scanPath = selected;
    }
    navigate("/disk", {
      state: {
        disk: scanPath,
        used: usedSpace,
        fullscan: true,
        isDirectory: isMacAppStore,
      },
    });
  };

  return (
    <div
      role="button"
      tabIndex={0}
      aria-label={`Scan ${disk.name || "Local Disk"}`}
      onContextMenu={(e) => {
        e.preventDefault();
        void scanDisk();
      }}
      onClick={() => void scanDisk()}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          void scanDisk();
        }
      }}
      className="storage-row group"
    >
      <div className="storage-icon storage-icon-local">
        <img src={icona} alt="" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-start justify-between gap-6">
          <div className="min-w-0">
            <div className="flex min-w-0 items-baseline gap-2">
              <span className="truncate text-[14px] font-semibold text-[#f4f6f7]">
                {disk.name ? disk.name : "Local Disk"}
              </span>
              <span className="truncate text-[11px] text-[#7f8993]">
                {disk.sMountPoint}
              </span>
            </div>
            <div className="mt-1 text-[12px] tabular-nums text-[#a6afb8]">
              {formatBytes(usedSpace)} of {formatBytes(disk.totalSpace)} used
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-4">
            <div className="text-right tabular-nums">
              <div className="text-[13px] font-semibold text-[#eef1f3]">
                {(perc * 100).toFixed(0)}%
              </div>
              <div className="mt-0.5 text-[11px] text-[#7f8993]">
                {formatBytes(disk.availableSpace)} free
              </div>
            </div>
            <ChevronRight className="storage-chevron" size={17} />
          </div>
        </div>
        <div className="storage-meter" data-tone={usageTone}>
          <div style={{ width: `${Math.min(100, perc * 100)}%` }} />
        </div>
      </div>
    </div>
  );
};

export default DiskItem;
