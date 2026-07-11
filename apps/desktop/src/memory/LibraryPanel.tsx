import { Card } from "../components/Card";

/**
 * The Library: browse and search everything your agent has remembered — captured
 * sessions and the notes it has kept.
 *
 * This is a minimal shell. Task C4 fills it in: the search bar, the session and note
 * lists, the reader (with Delete / note Supersede), and the RecallStats strip all live
 * inside this component. The heading + empty state below mark where that content goes.
 * Tokens only (no hardcoded colors) per the shell-redesign gate.
 */
export function LibraryPanel() {
  return (
    <div>
      <h2 style={{ margin: "0 0 8px" }}>Library</h2>
      <p style={{ color: "var(--text-secondary)", fontSize: 13, margin: "0 0 12px", lineHeight: 1.5 }}>
        Everything your agent remembers, in one place — search past sessions and the notes it has kept.
      </p>
      <Card>
        <p style={{ color: "var(--text-secondary)", fontSize: 13, margin: 0 }}>
          Your captured sessions and notes will appear here.
        </p>
      </Card>
    </div>
  );
}
