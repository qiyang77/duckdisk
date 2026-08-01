import Logo from "../assets/duck.png";
import { Link, useLocation } from "react-router-dom";
import { appWindow } from "@tauri-apps/api/window";
import { ChevronRight, Maximize2, Minus, X } from "lucide-react";

const WindowControls = () => {
  return (
    <div className="window-controls" aria-label="Window controls">
      <button
        type="button"
        className="window-control window-control-close"
        title="Close"
        aria-label="Close DuckDisk"
        onClick={() => appWindow.close()}
      >
        <X size={8} strokeWidth={2.4} />
      </button>
      <button
        type="button"
        className="window-control window-control-minimize"
        title="Minimize"
        aria-label="Minimize DuckDisk"
        onClick={() => appWindow.minimize()}
      >
        <Minus size={8} strokeWidth={2.4} />
      </button>
      <button
        type="button"
        className="window-control window-control-zoom"
        title="Zoom"
        aria-label="Zoom DuckDisk"
        onClick={() => appWindow.toggleMaximize()}
      >
        <Maximize2 size={7} strokeWidth={2.4} />
      </button>
    </div>
  );
};

const TitleBar = () => {
  let { state, pathname } = useLocation() as any;
  return (
    <div
      data-tauri-drag-region
      className="titlebar app-titlebar"
      onDoubleClick={(event) => {
        if (!(event.target as HTMLElement).closest("button, a")) {
          appWindow.toggleMaximize();
        }
      }}
    >
      <WindowControls />
      <nav className="navi titlebar-breadcrumbs" aria-label="Breadcrumb">
        <ol className="flex min-w-0 items-center">
          <li className="inline-flex min-w-0 items-center">
            {pathname === "/" ? (
              <span className="titlebar-product">DuckDisk</span>
            ) : (
              <Link
                to="/"
                className="titlebar-link titlebar-product"
              >
                DuckDisk
              </Link>
            )}
          </li>

          {pathname === "/disk" && (
            <li className="titlebar-crumb">
              <ChevronRight size={15} strokeWidth={2.2} />
              <Link to="/" className="titlebar-link">
                All Disks
              </Link>
            </li>
          )}
          {state && state.disk && (
            <li className="titlebar-crumb min-w-0" aria-current="page">
              <ChevronRight size={15} strokeWidth={2.2} />
              <span className="truncate text-[12px] font-medium text-[#7f8993]">
                {state.source && state.source !== "local"
                  ? state.disk
                  : `${state.isDirectory ? "Folder" : "Disk"} (${state.disk})`}
              </span>
            </li>
          )}
        </ol>
      </nav>
      <div className="titlebar-brand" aria-hidden="true">
        <img src={Logo} alt="" />
      </div>
    </div>
  );
};

export default TitleBar;
