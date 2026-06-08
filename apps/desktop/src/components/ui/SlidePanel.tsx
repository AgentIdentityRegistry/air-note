import { type ReactNode, useEffect } from "react";

type SlidePanelProps = {
  isOpen: boolean;
  onClose: () => void;
  title?: string;
  children: ReactNode;
};

export function SlidePanel({ isOpen, onClose, title, children }: SlidePanelProps) {
  useEffect(() => {
    if (!isOpen) {
      return;
    }

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [isOpen, onClose]);

  if (!isOpen) {
    return null;
  }

  return (
    <div className="slide-panel-backdrop" onMouseDown={onClose} role="presentation">
      <aside
        className="slide-panel"
        role="dialog"
        aria-modal="true"
        aria-label={title ?? "Panel"}
        onMouseDown={(event) => event.stopPropagation()}
      >
        {children}
      </aside>
    </div>
  );
}
