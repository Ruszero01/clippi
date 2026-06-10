# Sync Backend Counts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Display the current backend snapshot counts consistently across devices without treating no-op syncs as writes.

**Architecture:** Replace the ambiguous pushed-count tuple with a sync-cycle result that carries snapshot counts and a separate backend-write flag. Apply snapshot counts after every successful cycle that builds a snapshot, while preserving the existing timestamp update conditions.

**Tech Stack:** Rust, existing `SyncBackend` abstraction, `cargo test`

---

### Task 1: Add the regression test

**Files:**
- Modify: `src/services/gpui_sync.rs`

- [ ] Add a test-only in-memory backend returning a payload identical to the local database.
- [ ] Assert that the cycle reports the full snapshot item count and no backend write.
- [ ] Run `cargo test services::gpui_sync::tests::unchanged_snapshot_reports_backend_counts -- --exact` and confirm the assertion fails because the current cycle returns zero counts.

### Task 2: Separate snapshot counts from writes

**Files:**
- Modify: `src/services/gpui_sync.rs`

- [ ] Introduce a `SyncCycleResult` carrying success, message, merge statistics, snapshot counts, and `did_push`.
- [ ] Return snapshot counts when semantic hashes match, with `did_push = false`.
- [ ] Update `apply_result` to always persist successful snapshot counts and update `last_sync_at` only for merges or actual pushes.
- [ ] Run the focused regression test and confirm it passes.

### Task 3: Verify the change

**Files:**
- Modify: `src/services/gpui_sync.rs`

- [ ] Run `cargo fmt --check`.
- [ ] Run `cargo test`.
- [ ] Inspect `git diff --check` and the final diff for unrelated changes.
