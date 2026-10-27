import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

type Props = {
  label: ReactNode;
  title?: string;
  /** Renders the popover body; call `close` after an action completes. */
  children: (close: () => void) => ReactNode;
  align?: "left" | "right";
  /** Class of the toggle button. */
  buttonClass?: string;
};

/** Space kept between a panel and the window's edges. */
const MARGIN = 8;

/** A button that toggles a small panel below it. Closes on outside click or Esc. */
export function Popover({ label, title, children, align = "left", buttonClass }: Props) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const panel = useRef<HTMLDivElement>(null);

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

  // Keeps the panel inside the window: shifted sideways, and no taller than the space below.
  useLayoutEffect(() => {
    const el = panel.current;
    if (!open || !el) return;
    const fit = () => {
      el.style.transform = "";
      const r = el.getBoundingClientRect();
      const dx =
        r.right > window.innerWidth - MARGIN
          ? Math.max(MARGIN - r.left, window.innerWidth - MARGIN - r.right)
          : r.left < MARGIN
            ? MARGIN - r.left
            : 0;
      if (dx) el.style.transform = `translateX(${dx}px)`;
      el.style.maxHeight = `${window.innerHeight - r.top - MARGIN}px`;
    };
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(el);
    window.addEventListener("resize", fit);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", fit);
    };
  }, [open]);

  return (
    <div className="popover-anchor" ref={ref}>
      <button
        className={`${buttonClass ?? ""}${open ? " open" : ""}`.trim() || undefined}
        onClick={() => setOpen((o) => !o)}
        title={title}
        aria-expanded={open}
      >
        {label}
      </button>
      {open && (
        <div className={`popover ${align}`} ref={panel}>
          {children(() => setOpen(false))}
        </div>
      )}
    </div>
  );
}
