import type { ReactNode } from "react";

type SettingsSectionCardProps = {
  title: string;
  description?: string;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
};

export function SettingsSectionCard({
  title,
  description,
  actions,
  children,
  className
}: SettingsSectionCardProps) {
  return (
    <article className={["settings-section-card", className].filter(Boolean).join(" ")}>
      <div className="settings-section-card-head">
        <div>
          <h3>{title}</h3>
          {description ? <p className="muted">{description}</p> : null}
        </div>
        {actions ? <div className="settings-section-card-actions">{actions}</div> : null}
      </div>
      <div className="settings-section-card-body">{children}</div>
    </article>
  );
}

