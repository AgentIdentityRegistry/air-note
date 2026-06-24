import { useEffect, useState } from "react";
import { listProposals } from "../api/engine";

/** How often the Review nav badge refreshes its pending count. */
const REVIEW_POLL_MS = 5000;

/**
 * Pending-proposal count for the Review nav badge.
 * Polls every REVIEW_POLL_MS while `identityPresent` (no engine exists before onboarding).
 */
export function useReviewCount(identityPresent: boolean): number {
  const [count, setCount] = useState(0);
  useEffect(() => {
    if (!identityPresent) {
      setCount(0);
      return;
    }
    let alive = true;
    const refresh = () => {
      listProposals()
        .then((ps) => { if (alive) setCount(ps.length); })
        .catch(() => { if (alive) setCount(0); });
    };
    refresh();
    const id = setInterval(refresh, REVIEW_POLL_MS);
    return () => { alive = false; clearInterval(id); };
  }, [identityPresent]);
  return count;
}
