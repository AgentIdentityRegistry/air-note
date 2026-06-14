use air_rs::inbox::policy_store::{
    autonomy_for, cancel, confirm, decide, load, reserve, reset_to_draft, set_autonomy, AutoSend,
    Autonomy, ContactPolicy, Policy, GLOBAL_DAILY, PER_CONTACT_DAILY, PER_CONTACT_HOURLY,
};
use std::fs;
use tempfile::TempDir;

// ── Phase B test helpers ────────────────────────────────────────────────────────────────────────

const DID: &str = "did:x";
const NOW: &str = "2026-06-14T12:00:00Z";

/// An RFC-3339 timestamp `secs` seconds before [`NOW`].
fn secs_ago(secs: i64) -> String {
    let now = chrono::DateTime::parse_from_rfc3339(NOW).unwrap();
    (now - chrono::Duration::seconds(secs)).to_rfc3339()
}

/// A `Policy` with one contact at the given dial.
fn policy_with(dial: Autonomy) -> Policy {
    let mut p = Policy::default();
    p.contacts.insert(
        DID.to_string(),
        ContactPolicy {
            ai_autonomy: dial,
            ..Default::default()
        },
    );
    p
}

/// An `AutoSend` confirmed at `at` (a fresh uuid-free token is fine for `decide`-only tests).
fn confirmed_at(thread: &str, at: &str, token: &str) -> AutoSend {
    AutoSend {
        token: token.to_string(),
        envelope_id: format!("env-{token}"),
        thread_id: thread.to_string(),
        at: at.to_string(),
        pending: false,
    }
}

#[test]
fn missing_file_is_all_draft() {
    let h = TempDir::new().unwrap();
    assert_eq!(autonomy_for(h.path(), "did:x"), Autonomy::Draft);
}

#[test]
fn corrupt_file_is_all_draft() {
    let h = TempDir::new().unwrap();
    fs::write(h.path().join("agent-policy.json"), "{ not json").unwrap();
    assert_eq!(autonomy_for(h.path(), "did:x"), Autonomy::Draft);
}

#[test]
fn set_then_read_round_trips_and_persists_0600() {
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), "did:x", Autonomy::Auto).unwrap();
    assert_eq!(autonomy_for(h.path(), "did:x"), Autonomy::Auto);
    assert_eq!(autonomy_for(h.path(), "did:other"), Autonomy::Draft);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(h.path().join("agent-policy.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let raw = fs::read_to_string(h.path().join("agent-policy.json")).unwrap();
    assert!(raw.contains("\"auto\""));
    let _ = load(h.path());
}

// ── decide: the pure decision table ──────────────────────────────────────────────────────────────

#[test]
fn decide_off_dial_returns_off() {
    let p = policy_with(Autonomy::Off);
    assert_eq!(decide(&p, DID, None, NOW, NOW), (Autonomy::Off, "dial off"));
}

#[test]
fn decide_draft_dial_returns_draft() {
    let p = policy_with(Autonomy::Draft);
    assert_eq!(
        decide(&p, DID, None, NOW, NOW),
        (Autonomy::Draft, "dial draft")
    );
}

#[test]
fn decide_unknown_contact_defaults_to_draft() {
    let p = Policy::default();
    assert_eq!(
        decide(&p, "did:nobody", None, NOW, NOW),
        (Autonomy::Draft, "dial draft")
    );
}

#[test]
fn decide_auto_clean_returns_auto() {
    let p = policy_with(Autonomy::Auto);
    assert_eq!(
        decide(&p, DID, Some("t1"), NOW, NOW),
        (Autonomy::Auto, "auto")
    );
}

#[test]
fn decide_auto_recency_old_message_is_draft() {
    // D11: received 11 min ago (> 10 min) → too old.
    let p = policy_with(Autonomy::Auto);
    let (d, r) = decide(&p, DID, Some("t1"), &secs_ago(660), NOW);
    assert_eq!(d, Autonomy::Draft);
    assert_eq!(r, "auto skipped: message too old (replay)");
}

#[test]
fn decide_auto_recency_boundary_exactly_10min_is_auto() {
    // within() is inclusive: exactly 600s old still passes.
    let p = policy_with(Autonomy::Auto);
    assert_eq!(
        decide(&p, DID, Some("t1"), &secs_ago(600), NOW),
        (Autonomy::Auto, "auto")
    );
}

#[test]
fn decide_structural_same_thread_recent_is_draft() {
    // D2: an auto_send in the same thread 10 min ago (< 30 min) pauses auto for that thread.
    let mut p = policy_with(Autonomy::Auto);
    p.contacts
        .get_mut(DID)
        .unwrap()
        .auto_sends
        .push(confirmed_at("t1", &secs_ago(600), "a"));
    let (d, r) = decide(&p, DID, Some("t1"), NOW, NOW);
    assert_eq!(d, Autonomy::Draft);
    assert_eq!(r, "auto paused: just auto-replied in this thread");
}

#[test]
fn decide_structural_different_thread_is_auto() {
    // A recent auto_send in ANOTHER thread does not pause this thread.
    let mut p = policy_with(Autonomy::Auto);
    p.contacts
        .get_mut(DID)
        .unwrap()
        .auto_sends
        .push(confirmed_at("other", &secs_ago(600), "a"));
    assert_eq!(
        decide(&p, DID, Some("t1"), NOW, NOW),
        (Autonomy::Auto, "auto")
    );
}

#[test]
fn decide_structural_old_same_thread_is_auto() {
    // Same thread but 31 min ago (> 30 min window) → structural guard lifted.
    let mut p = policy_with(Autonomy::Auto);
    p.contacts
        .get_mut(DID)
        .unwrap()
        .auto_sends
        .push(confirmed_at("t1", &secs_ago(1860), "a"));
    assert_eq!(
        decide(&p, DID, Some("t1"), NOW, NOW),
        (Autonomy::Auto, "auto")
    );
}

#[test]
fn decide_hourly_cap_is_draft() {
    // 3 auto_sends in the last hour (different threads, so D2 doesn't fire) → hourly cap.
    let mut p = policy_with(Autonomy::Auto);
    let sends = &mut p.contacts.get_mut(DID).unwrap().auto_sends;
    for i in 0..PER_CONTACT_HOURLY {
        sends.push(confirmed_at(
            &format!("h{i}"),
            &secs_ago(60),
            &format!("h{i}"),
        ));
    }
    let (d, r) = decide(&p, DID, Some("new"), NOW, NOW);
    assert_eq!(d, Autonomy::Draft);
    assert_eq!(r, "auto paused: hourly cap");
}

#[test]
fn decide_daily_cap_is_draft() {
    // 10 auto_sends spread across the last 24h but only 2 in the last hour → hourly OK, daily cap.
    let mut p = policy_with(Autonomy::Auto);
    let sends = &mut p.contacts.get_mut(DID).unwrap().auto_sends;
    // 2 within the hour (under the hourly cap of 3)…
    sends.push(confirmed_at("d0", &secs_ago(60), "d0"));
    sends.push(confirmed_at("d1", &secs_ago(120), "d1"));
    // …plus 8 more older than an hour but within 24h → 10 total in 24h.
    for i in 2..PER_CONTACT_DAILY {
        sends.push(confirmed_at(
            &format!("d{i}"),
            &secs_ago(3600 + i as i64 * 100),
            &format!("d{i}"),
        ));
    }
    let (d, r) = decide(&p, DID, Some("new"), NOW, NOW);
    assert_eq!(d, Autonomy::Draft);
    assert_eq!(r, "auto paused: daily cap");
}

#[test]
fn decide_global_cap_is_draft() {
    // 30 auto_sends across MANY contacts in 24h (≤2 each, so no per-contact cap fires) → global cap.
    let mut p = Policy::default();
    p.contacts.insert(
        DID.to_string(),
        ContactPolicy {
            ai_autonomy: Autonomy::Auto,
            ..Default::default()
        },
    );
    let mut placed = 0;
    let mut c = 0;
    while placed < GLOBAL_DAILY {
        let did = format!("did:bulk{c}");
        let entry = p.contacts.entry(did).or_default();
        // 2 per bulk contact, older than an hour to avoid the structural/hourly interplay on DID.
        entry.auto_sends.push(confirmed_at(
            "g",
            &secs_ago(7200 + c as i64),
            &format!("g{c}a"),
        ));
        entry.auto_sends.push(confirmed_at(
            "g",
            &secs_ago(7300 + c as i64),
            &format!("g{c}b"),
        ));
        placed += 2;
        c += 1;
    }
    let (d, r) = decide(&p, DID, Some("new"), NOW, NOW);
    assert_eq!(d, Autonomy::Draft);
    assert_eq!(r, "auto paused: global cap");
}

#[test]
fn decide_budget_counts_pending_too() {
    // 3 PENDING reservations (no envelope_id yet) still hit the hourly cap — the C2-blocking behavior.
    let mut p = policy_with(Autonomy::Auto);
    let sends = &mut p.contacts.get_mut(DID).unwrap().auto_sends;
    for i in 0..PER_CONTACT_HOURLY {
        sends.push(AutoSend {
            token: format!("p{i}"),
            envelope_id: String::new(),
            thread_id: format!("p{i}"),
            at: secs_ago(30),
            pending: true,
        });
    }
    assert_eq!(decide(&p, DID, Some("new"), NOW, NOW).0, Autonomy::Draft);
}

// ── reserve / confirm / cancel: the atomic mutators ──────────────────────────────────────────────

#[test]
fn reserve_burst_respects_hourly_cap_via_pending() {
    // THE C2 fix: 4 reserves for the same contact under hourly cap 3 → first 3 Auto+token, 4th Draft.
    // The pending reservations written by the first 3 are what the 4th's decide() counts.
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), DID, Autonomy::Auto).unwrap();

    let mut tokens = Vec::new();
    for i in 0..PER_CONTACT_HOURLY {
        // Distinct thread per reserve so the D2 structural guard never fires — this isolates the
        // BUDGET path, which is the C2 behavior under test.
        let res = reserve(h.path(), DID, Some(&format!("thread-{i}")), NOW, NOW).unwrap();
        assert_eq!(res.decision, Autonomy::Auto, "reason was: {}", res.reason);
        tokens.push(res.token.expect("auto reserve yields a token"));
    }
    let fourth = reserve(h.path(), DID, Some("thread-z"), NOW, NOW).unwrap();
    assert_eq!(fourth.decision, Autonomy::Draft);
    assert_eq!(fourth.reason, "auto paused: hourly cap");
    assert!(fourth.token.is_none());

    // Exactly 3 pending reservations persisted.
    let p = load(h.path());
    let sends = &p.contacts.get(DID).unwrap().auto_sends;
    assert_eq!(sends.len(), PER_CONTACT_HOURLY);
    assert!(sends.iter().all(|a| a.pending));
    assert_eq!(tokens.len(), 3);
}

#[test]
fn reserve_burst_blocks_even_with_same_thread() {
    // Structural guard (D2) keys off a PRIOR record's at; the first same-thread reserve writes a
    // record at NOW, so the second same-thread reserve is paused structurally (not budget). Prove
    // the very first wins and the second is Draft for the structural reason.
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), DID, Autonomy::Auto).unwrap();
    let first = reserve(h.path(), DID, Some("t1"), NOW, NOW).unwrap();
    assert_eq!(first.decision, Autonomy::Auto);
    let second = reserve(h.path(), DID, Some("t1"), NOW, NOW).unwrap();
    assert_eq!(second.decision, Autonomy::Draft);
    assert_eq!(
        second.reason,
        "auto paused: just auto-replied in this thread"
    );
}

#[test]
fn confirm_flips_pending_and_records_ledger() {
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), DID, Autonomy::Auto).unwrap();
    let token = reserve(h.path(), DID, Some("t1"), NOW, NOW)
        .unwrap()
        .token
        .unwrap();

    confirm(h.path(), DID, &token, "env-123", "t1", NOW).unwrap();

    let p = load(h.path());
    let c = p.contacts.get(DID).unwrap();
    let rec = c.auto_sends.iter().find(|a| a.token == token).unwrap();
    assert!(!rec.pending);
    assert_eq!(rec.envelope_id, "env-123");
    assert_eq!(c.auto_ledger, vec!["env-123".to_string()]);
}

#[test]
fn confirm_reinserts_when_pending_was_pruned() {
    // I-A: the pending record is gone (ack arrived after a prune). confirm must restore a CONFIRMED
    // record so the send counts toward budget + structural guard, and lands in the ledger.
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), DID, Autonomy::Auto).unwrap();
    // No reserve at all → token never existed on disk; confirm restores it.
    confirm(h.path(), DID, "ghost-token", "env-restored", "t9", NOW).unwrap();

    let p = load(h.path());
    let c = p.contacts.get(DID).unwrap();
    let rec = c
        .auto_sends
        .iter()
        .find(|a| a.token == "ghost-token")
        .unwrap();
    assert!(!rec.pending);
    assert_eq!(rec.envelope_id, "env-restored");
    assert_eq!(rec.thread_id, "t9");
    assert_eq!(rec.at, NOW);
    assert_eq!(c.auto_ledger, vec!["env-restored".to_string()]);
}

#[test]
fn confirm_caps_ledger_at_500() {
    let h = TempDir::new().unwrap();
    // Pre-seed a ledger already at the cap, then confirm one more.
    let mut p = policy_with(Autonomy::Auto);
    let c = p.contacts.get_mut(DID).unwrap();
    c.auto_ledger = (0..500).map(|i| format!("old-{i}")).collect();
    write_policy(h.path(), &p);

    confirm(h.path(), DID, "tok", "env-newest", "t1", NOW).unwrap();

    let after = load(h.path());
    let ledger = &after.contacts.get(DID).unwrap().auto_ledger;
    assert_eq!(ledger.len(), 500);
    assert_eq!(ledger.last().unwrap(), "env-newest");
    // The oldest entry was dropped.
    assert_eq!(ledger.first().unwrap(), "old-1");
}

#[test]
fn cancel_removes_pending_and_frees_budget() {
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), DID, Autonomy::Auto).unwrap();
    let token = reserve(h.path(), DID, Some("t1"), NOW, NOW)
        .unwrap()
        .token
        .unwrap();

    cancel(h.path(), DID, &token).unwrap();

    let p = load(h.path());
    assert!(p.contacts.get(DID).unwrap().auto_sends.is_empty());
}

#[test]
fn cancel_does_not_remove_confirmed() {
    // M-A: a late cancel for a token that was already confirmed must NOT delete the confirmed send.
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), DID, Autonomy::Auto).unwrap();
    let token = reserve(h.path(), DID, Some("t1"), NOW, NOW)
        .unwrap()
        .token
        .unwrap();
    confirm(h.path(), DID, &token, "env-keep", "t1", NOW).unwrap();

    cancel(h.path(), DID, &token).unwrap();

    let p = load(h.path());
    let sends = &p.contacts.get(DID).unwrap().auto_sends;
    assert_eq!(sends.len(), 1);
    assert!(!sends[0].pending);
    assert_eq!(sends[0].envelope_id, "env-keep");
}

// ── prune: retention + stale-pending crash-safety ────────────────────────────────────────────────

#[test]
fn prune_via_reserve_drops_old_and_stale_keeps_recent() {
    // prune() is private; exercise it through reserve()'s write path. Seed a mix, then reserve once
    // (a clean Auto in a fresh thread) and assert what survives.
    let h = TempDir::new().unwrap();
    let mut p = policy_with(Autonomy::Auto);
    let sends = &mut p.contacts.get_mut(DID).unwrap().auto_sends;
    sends.push(confirmed_at("t-old", &secs_ago(90_000), "old")); // > 24h → dropped
    sends.push(AutoSend {
        token: "stale".into(),
        envelope_id: String::new(),
        thread_id: "t-stale".into(),
        // 2h ago: pending & > 15m → dropped by the stale rule, yet < 24h so it proves the stale
        // rule (not retention) is what removes it. Placed outside the hour so it doesn't inflate
        // the hourly count that the reserve's pure decide() sees before pruning.
        at: secs_ago(7200),
        pending: true,
    });
    sends.push(confirmed_at("t-recent", &secs_ago(60), "recent")); // confirmed, recent → kept
    sends.push(AutoSend {
        token: "fresh".into(),
        envelope_id: String::new(),
        thread_id: "t-fresh".into(),
        at: secs_ago(60), // pending & < 15m → kept
        pending: true,
    });
    write_policy(h.path(), &p);

    // Reserve in a brand-new thread so the only effect is the prune + the new pending record.
    let res = reserve(h.path(), DID, Some("t-brand-new"), NOW, NOW).unwrap();
    assert_eq!(res.decision, Autonomy::Auto, "reason: {}", res.reason);

    let after = load(h.path());
    let tokens: Vec<&str> = after
        .contacts
        .get(DID)
        .unwrap()
        .auto_sends
        .iter()
        .map(|a| a.token.as_str())
        .collect();
    assert!(
        tokens.contains(&"recent"),
        "recent confirmed should survive"
    );
    assert!(tokens.contains(&"fresh"), "fresh pending should survive");
    assert!(!tokens.contains(&"old"), "old (>24h) should be pruned");
    assert!(
        !tokens.contains(&"stale"),
        "stale pending (>15m) should be pruned"
    );
    // plus the newly reserved one.
    assert_eq!(after.contacts.get(DID).unwrap().auto_sends.len(), 3);
}

// ── within boundary (observed through decide, which is the only public surface) ───────────────────

#[test]
fn within_is_inclusive_at_the_window_edge() {
    // A record exactly at the structural window edge (1800s) still counts → Draft.
    let mut p = policy_with(Autonomy::Auto);
    p.contacts
        .get_mut(DID)
        .unwrap()
        .auto_sends
        .push(confirmed_at("t1", &secs_ago(1800), "edge"));
    let (d, _) = decide(&p, DID, Some("t1"), NOW, NOW);
    assert_eq!(
        d,
        Autonomy::Draft,
        "exactly-at-edge must count (inclusive <=)"
    );
    // One second past the edge → no longer counts.
    let mut p2 = policy_with(Autonomy::Auto);
    p2.contacts
        .get_mut(DID)
        .unwrap()
        .auto_sends
        .push(confirmed_at("t1", &secs_ago(1801), "past"));
    assert_eq!(decide(&p2, DID, Some("t1"), NOW, NOW).0, Autonomy::Auto);
}

// ── reset_to_draft + lock-shared dial path ───────────────────────────────────────────────────────

#[test]
fn reset_to_draft_forces_draft() {
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), DID, Autonomy::Auto).unwrap();
    assert_eq!(autonomy_for(h.path(), DID), Autonomy::Auto);
    reset_to_draft(h.path(), DID).unwrap();
    assert_eq!(autonomy_for(h.path(), DID), Autonomy::Draft);
}

#[test]
fn reset_to_draft_preserves_auto_sends() {
    // Re-pin resets the dial but must not wipe the audit/budget history.
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), DID, Autonomy::Auto).unwrap();
    let token = reserve(h.path(), DID, Some("t1"), NOW, NOW)
        .unwrap()
        .token
        .unwrap();
    confirm(h.path(), DID, &token, "env-x", "t1", NOW).unwrap();
    reset_to_draft(h.path(), DID).unwrap();
    let p = load(h.path());
    let c = p.contacts.get(DID).unwrap();
    assert_eq!(c.ai_autonomy, Autonomy::Draft);
    assert_eq!(c.auto_ledger, vec!["env-x".to_string()]);
    assert_eq!(c.auto_sends.len(), 1);
}

// ── backward-compat: old file with no auto_sends key still loads ──────────────────────────────────

#[test]
fn old_policy_without_auto_sends_deserializes() {
    let h = TempDir::new().unwrap();
    fs::write(
        h.path().join("agent-policy.json"),
        r#"{"version":1,"contacts":{"did:x":{"ai_autonomy":"auto","auto_ledger":["e1"]}}}"#,
    )
    .unwrap();
    let p = load(h.path());
    let c = p.contacts.get("did:x").unwrap();
    assert_eq!(c.ai_autonomy, Autonomy::Auto);
    assert_eq!(c.auto_ledger, vec!["e1".to_string()]);
    assert!(c.auto_sends.is_empty()); // serde default
}

// ── FIX 1: the stale-pending wedge self-heals (prune-before-decide) ───────────────────────────────

#[test]
fn reserve_self_heals_from_stale_pending_wedge() {
    // Regression: 3 stale pending reservations (~20m old, controller died before their acks) had
    // wedged the contact at Draft — they inflated decide()'s hourly count, forcing Draft, so the
    // old Auto-only prune never ran. With prune-before-decide, reserve sees honest (zero) counts and
    // returns Auto, and the 3 stale pendings are gone afterward (only the new one remains).
    let h = TempDir::new().unwrap();
    let mut p = policy_with(Autonomy::Auto);
    let sends = &mut p.contacts.get_mut(DID).unwrap().auto_sends;
    for i in 0..PER_CONTACT_HOURLY {
        sends.push(AutoSend {
            token: format!("wedge{i}"),
            envelope_id: String::new(),
            thread_id: format!("wedge-thread-{i}"),
            at: secs_ago(1200), // ~20 min → pending & > 15m stale
            pending: true,
        });
    }
    write_policy(h.path(), &p);

    let res = reserve(h.path(), DID, Some("fresh-thread"), NOW, NOW).unwrap();
    assert_eq!(
        res.decision,
        Autonomy::Auto,
        "wedge must self-heal; reason: {}",
        res.reason
    );

    let after = load(h.path());
    let sends = &after.contacts.get(DID).unwrap().auto_sends;
    assert_eq!(sends.len(), 1, "only the new reservation should remain");
    assert!(
        !sends[0].token.starts_with("wedge"),
        "stale pendings must be gone"
    );
    assert!(sends[0].pending);
}

#[test]
fn reserve_draft_path_flushes_prune_to_disk() {
    // Even when the decision is Draft (dial off), a prune that removed stale debris must be
    // persisted — otherwise the file never self-heals on non-Auto reserves.
    let h = TempDir::new().unwrap();
    let mut p = policy_with(Autonomy::Off); // dial off → decide returns Draft, never Auto
    p.contacts.get_mut(DID).unwrap().auto_sends.push(AutoSend {
        token: "debris".into(),
        envelope_id: String::new(),
        thread_id: "t".into(),
        at: secs_ago(1200), // stale pending
        pending: true,
    });
    write_policy(h.path(), &p);

    let res = reserve(h.path(), DID, Some("t"), NOW, NOW).unwrap();
    assert_eq!(res.decision, Autonomy::Off);
    assert!(res.token.is_none());

    // The stale debris was flushed away despite the non-Auto path.
    let after = load(h.path());
    assert!(after.contacts.get(DID).unwrap().auto_sends.is_empty());
}

// ── FIX 2: within() fails closed on future timestamps ─────────────────────────────────────────────

#[test]
fn decide_future_received_at_is_too_old_not_auto() {
    // A future-dated message must NOT bypass the recency cutoff. 5 min in the FUTURE → out-of-window
    // → "too old" → Draft (fail-closed).
    let p = policy_with(Autonomy::Auto);
    let future = secs_ago(-300); // 5 min ahead of NOW
    let (d, r) = decide(&p, DID, Some("t1"), &future, NOW);
    assert_eq!(d, Autonomy::Draft);
    assert_eq!(r, "auto skipped: message too old (replay)");
}

#[test]
fn decide_future_auto_send_does_not_count_toward_caps() {
    // A future-stamped auto_send record (clock skew / tampering) is out-of-window for every cap, so
    // it can't be used to either inflate OR (here) be ignored maliciously — it simply doesn't count.
    // With 2 real recent + 1 future record, the hourly count is 2 (not 3) → still Auto.
    let mut p = policy_with(Autonomy::Auto);
    let sends = &mut p.contacts.get_mut(DID).unwrap().auto_sends;
    sends.push(confirmed_at("a", &secs_ago(60), "a"));
    sends.push(confirmed_at("b", &secs_ago(120), "b"));
    sends.push(confirmed_at("c", &secs_ago(-600), "c")); // future → not counted
    assert_eq!(
        decide(&p, DID, Some("new"), NOW, NOW),
        (Autonomy::Auto, "auto")
    );
}

// ── FIX 3: confirm is idempotent for the ledger (at-least-once delivery) ───────────────────────────

#[test]
fn confirm_is_idempotent_on_duplicate_ack() {
    // A duplicate inbox_send_ok (delivery is at-least-once) must not double-write the ledger.
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), DID, Autonomy::Auto).unwrap();
    let token = reserve(h.path(), DID, Some("t1"), NOW, NOW)
        .unwrap()
        .token
        .unwrap();

    confirm(h.path(), DID, &token, "env-dup", "t1", NOW).unwrap();
    confirm(h.path(), DID, &token, "env-dup", "t1", NOW).unwrap(); // duplicate ack

    let p = load(h.path());
    let c = p.contacts.get(DID).unwrap();
    // Ledger has the envelope_id exactly once…
    assert_eq!(c.auto_ledger, vec!["env-dup".to_string()]);
    // …and there's still exactly one auto_send record (the confirmed one).
    assert_eq!(c.auto_sends.len(), 1);
    assert!(!c.auto_sends[0].pending);
}

// ── FIX 6: overlapping writers don't corrupt the file (unique tmp) ────────────────────────────────

#[test]
fn concurrent_confirms_do_not_corrupt_the_file() {
    // Unique tmp names mean overlapping write_atomic calls can't collide on a shared tmp (which
    // previously caused torn files / ENOENT renames). After a burst of concurrent confirms the file
    // must still parse as valid JSON and every confirmed envelope must be in the ledger exactly once.
    use std::sync::Arc;
    use std::thread;

    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), DID, Autonomy::Auto).unwrap();
    let home: Arc<std::path::PathBuf> = Arc::new(h.path().to_path_buf());

    let n = 12;
    let handles: Vec<_> = (0..n)
        .map(|i| {
            let home = Arc::clone(&home);
            thread::spawn(move || {
                // Each confirm uses a distinct token+envelope; the not-found arm re-inserts a
                // confirmed record (I-A), so all N should land without any corruption.
                confirm(
                    &home,
                    DID,
                    &format!("tok-{i}"),
                    &format!("env-{i}"),
                    "t",
                    NOW,
                )
                .unwrap();
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    // The file is still valid and complete.
    let p = load(home.as_path());
    let c = p.contacts.get(DID).unwrap();
    assert_eq!(
        c.auto_ledger.len(),
        n,
        "every confirm should be recorded exactly once"
    );
    for i in 0..n {
        assert!(
            c.auto_ledger.contains(&format!("env-{i}")),
            "env-{i} missing from ledger"
        );
    }
    // No leftover tmp files in the home dir.
    let leftover_tmp: Vec<_> = fs::read_dir(home.as_path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(
        leftover_tmp.is_empty(),
        "no .tmp files should remain: {leftover_tmp:?}"
    );
}

/// Write a `Policy` straight to `home` for seeding tests (mirrors the crate's atomic write but is
/// test-local so we don't expose the private `write_atomic`).
fn write_policy(home: &std::path::Path, p: &Policy) {
    fs::create_dir_all(home).unwrap();
    let json = serde_json::to_string_pretty(p).unwrap();
    fs::write(home.join("agent-policy.json"), json).unwrap();
}

// ── C2: real concurrent reserves cannot both pass the cap ────────────────────────────────────────

#[test]
fn concurrent_reserves_never_exceed_hourly_cap() {
    // Spin up many threads racing to reserve for the SAME contact in DISTINCT threads (so only the
    // budget gate applies). POLICY_LOCK must serialize load→decide→write, so the number of granted
    // Auto tokens is EXACTLY the hourly cap — never more. Without the lock, two reserves could read
    // the same pre-state and both pass.
    use std::sync::Arc;
    use std::thread;

    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), DID, Autonomy::Auto).unwrap();
    let home: Arc<std::path::PathBuf> = Arc::new(h.path().to_path_buf());

    let racers = 16;
    let handles: Vec<_> = (0..racers)
        .map(|i| {
            let home = Arc::clone(&home);
            thread::spawn(move || {
                reserve(&home, DID, Some(&format!("c-thread-{i}")), NOW, NOW)
                    .unwrap()
                    .token
                    .is_some()
            })
        })
        .collect();

    // `join()` consumes each handle, so map (not filter) to owned bools, then count the winners.
    let granted = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|won| *won)
        .count();
    assert_eq!(
        granted, PER_CONTACT_HOURLY,
        "exactly the cap may win the race"
    );

    // And the persisted pending count matches — no extra slipped through to disk.
    let p = load(home.as_path());
    let pending = p
        .contacts
        .get(DID)
        .unwrap()
        .auto_sends
        .iter()
        .filter(|a| a.pending)
        .count();
    assert_eq!(pending, PER_CONTACT_HOURLY);
}
