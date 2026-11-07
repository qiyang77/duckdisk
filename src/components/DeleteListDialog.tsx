import { useEffect, useRef } from "react";
import { File as FileIcon, Folder as FolderIcon, X } from "lucide-react";

type Props = {
  items: DiskItem[];
  usesTrash: boolean;
  isDeleting: boolean;
  onClose: () => void;
  onRemove: (id: string) => void;
};

const DeleteListDialog = ({ items, usesTrash, isDeleting, onClose, onRemove }: Props) => {
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const previousFocus = document.activeElement;
    closeRef.current?.focus();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCloseRef.current();
      }
      if (event.key !== "Tab") return;
      const buttons = dialogRef.current?.querySelectorAll<HTMLButtonElement>(
        "button:not(:disabled)"
      );
      if (!buttons?.length) return;
      const first = buttons[0];
      const last = buttons[buttons.length - 1];
      const outside = !dialogRef.current?.contains(document.activeElement);
      if (outside || (event.shiftKey && document.activeElement === first)) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      if (previousFocus instanceof HTMLElement && previousFocus.isConnected) {
        previousFocus.focus();
      }
    };
  }, []);

  return (
    <div
      className="modal-backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="app-dialog delete-list-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="delete-list-title"
      >
        <div className="dialog-header">
          <div>
            <div id="delete-list-title" className="text-sm font-semibold text-white">
              {usesTrash ? "Trash List" : "Delete List"}
            </div>
            <div className="mt-1 text-xs text-slate-400">
              {items.length.toLocaleString()} item{items.length === 1 ? "" : "s"}
            </div>
          </div>
          <button
            ref={closeRef}
            type="button"
            className="icon-button"
            aria-label="Close candidate list"
            onClick={onClose}
          >
            <X size={16} />
          </button>
        </div>
        <ul className="delete-list-items" aria-label="Deletion candidates">
          {items.map((item) => (
            <li key={item.id} className="delete-list-item">
              {item.isDirectory || item.children?.length ? (
                <FolderIcon size={16} className="item-type-icon" />
              ) : (
                <FileIcon size={16} className="item-type-icon" />
              )}
              <div className="min-w-0 flex-1">
                <div className="delete-list-name">{item.name || item.id}</div>
                <div className="delete-list-path">{item.id}</div>
              </div>
              <button
                type="button"
                className="icon-button"
                aria-label={`Remove ${item.name || item.id} from candidate list`}
                title="Remove from list"
                disabled={isDeleting}
                onClick={(event) => {
                  onRemove(item.id);
                  // The row's remove button disappears; keep keyboard focus in the dialog.
                  if (event.detail === 0) closeRef.current?.focus();
                }}
              >
                <X size={14} />
              </button>
            </li>
          ))}
          {!items.length && <li className="delete-list-empty">No items in the list</li>}
        </ul>
      </div>
    </div>
  );
};

export default DeleteListDialog;
