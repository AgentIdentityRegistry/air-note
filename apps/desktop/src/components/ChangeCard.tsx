import type { ConfigChangeProposal } from "../models";

type ChangeCardProps = {
  proposal: ConfigChangeProposal;
  onApply: () => void;
  onCancel: () => void;
  onEdit?: () => void;
};

export function ChangeCard({ proposal, onApply, onCancel, onEdit }: ChangeCardProps) {
  return (
    <article className="entity-card">
      <h3>{proposal.summary}</h3>
      <p className="muted">This change will be logged and can be undone.</p>

      {proposal.diff.added.length ? (
        <div>
          <h4>Added</h4>
          <ul className="simple-list">
            {proposal.diff.added.map((entry, index) => (
              <li key={`${proposal.id}-added-${index}`}>{entry}</li>
            ))}
          </ul>
        </div>
      ) : null}

      {proposal.diff.removed.length ? (
        <div>
          <h4>Removed</h4>
          <ul className="simple-list">
            {proposal.diff.removed.map((entry, index) => (
              <li key={`${proposal.id}-removed-${index}`}>{entry}</li>
            ))}
          </ul>
        </div>
      ) : null}

      {proposal.diff.changed.length ? (
        <div>
          <h4>Changed</h4>
          <ul className="simple-list">
            {proposal.diff.changed.map((entry) => (
              <li key={`${proposal.id}-${entry.path}`}>
                <strong>{entry.path}</strong>: {entry.from} -&gt; {entry.to}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      <div className="row-actions">
        {onEdit ? (
          <button type="button" className="secondary-btn" onClick={onEdit}>
            Edit
          </button>
        ) : null}
        <button type="button" className="secondary-btn" onClick={onCancel}>
          Cancel
        </button>
        <button type="button" onClick={onApply}>
          {proposal.applyMode === "fsd" ? "Apply (FSD)" : "Apply"}
        </button>
      </div>
    </article>
  );
}
