import type { ReactNode } from "react";

type ToggleSwitchProps = {
  checked: boolean;
  onChange: (next: boolean) => void;
  label?: ReactNode;
  onLabel?: string;
  offLabel?: string;
  disabled?: boolean;
};

export function ToggleSwitch({
  checked,
  onChange,
  label,
  onLabel = "On",
  offLabel = "Off",
  disabled = false
}: ToggleSwitchProps) {
  return (
    <label className="toggle-wrap">
      {label ? <span className="toggle-label">{label}</span> : null}
      <button
        type="button"
        className={checked ? "toggle-switch on" : "toggle-switch off"}
        role="switch"
        aria-checked={checked}
        disabled={disabled}
        onClick={() => onChange(!checked)}
      >
        <span className="toggle-track" />
        <span className="toggle-thumb" />
        <span className="toggle-text">{checked ? onLabel : offLabel}</span>
      </button>
    </label>
  );
}
