import { useEffect, useRef, useState, type ReactNode } from "react";

type Props = {
  label: ReactNode;
  title?: string;
  /** Renders the popover body; call `close` after an action completes. */
  children: (close: () => void) => ReactNode;
  align?: "left" | "right";
};

/** A button that toggles a small panel below it. Closes on outside click or Esc. */
export function Popover({ label, title, children, align = "left" }: Props) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onPointer = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
      }
    };
    window.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  return (
    <div className="popover-anchor" ref={ref}>
      <button onClick={() => setOpen((o) => !o)} title={title} aria-expanded={open}>
        {label}
      </button>
      {open && <div className={`popover ${align}`}>{children(() => setOpen(false))}</div>}
    </div>
  );
}
