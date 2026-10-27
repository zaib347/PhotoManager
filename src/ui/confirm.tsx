import { useEffect, useRef } from "react";
import { createRoot } from "react-dom/client";
import { Icon } from "./Icon";

type Options = {
  title?: string;
  /** "warning" marks an action that removes or discards something. */
  kind?: "info" | "warning";
  okLabel?: string;
  cancelLabel?: string;
};

/**
 * Asks a yes/no question in a dialog drawn in the app's theme (the system one can't
 * follow it). Same call as the dialog plugin's `confirm`; resolves true for OK.
 */
export function confirm(message: string, options: Options = {}): Promise<boolean> {
  return new Promise((resolve) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    const previous = document.activeElement as HTMLElement | null;
    const close = (ok: boolean) => {
      root.unmount();
      host.remove();
      previous?.focus?.();
      resolve(ok);
    };
    root.render(<ConfirmDialog message={message} options={options} onClose={close} />);
  });
}

function ConfirmDialog({
  message,
  options,
  onClose,
}: {
  message: string;
  options: Options;
  onClose: (ok: boolean) => void;
}) {
  const ok = useRef<HTMLButtonElement>(null);
  const warning = options.kind === "warning";

  useEffect(() => {
    ok.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose(false);
      }
    };
    // Capture, so Escape closes the dialog rather than the page behind it.
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div className="confirm-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose(false)}>
      <div className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title">
        <div className="confirm-body">
          <span className={`confirm-icon${warning ? " warning" : ""}`}>
            <Icon name={warning ? "alert" : "info"} size={20} />
          </span>
          <div className="confirm-text">
            <h3 id="confirm-title">{options.title ?? "Confirm"}</h3>
            {message.split("\n\n").map((part, i) => (
              <p key={i} className={i === 0 ? "" : "muted"}>
                {part}
              </p>
            ))}
          </div>
        </div>
        <div className="confirm-actions">
          <button onClick={() => onClose(false)}>{options.cancelLabel ?? "Cancel"}</button>
          <button ref={ok} className={warning ? "danger-fill" : "primary"} onClick={() => onClose(true)}>
            {options.okLabel ?? "OK"}
          </button>
        </div>
      </div>
    </div>
  );
}
