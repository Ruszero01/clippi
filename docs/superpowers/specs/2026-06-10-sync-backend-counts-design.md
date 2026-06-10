# Sync Backend Counts Design

## Goal

Make every device display the item and tag counts of the backend's current
logical snapshot after a successful manual sync.

## Design

The sync cycle will report two independent facts:

- the item and tag counts in the final local snapshot after pulling and merging;
- whether that snapshot was actually written to the backend.

`BackendConfig.last_item_count` and `last_tag_count` will always be refreshed
from the final snapshot after a successful cycle that builds one. The
`last_sync_at` timestamp will continue to change only when remote data was
merged locally or a backend write occurred. This preserves the existing
protection against showing "just now" for no-op syncs.

The cloud payload format and merge behavior are unchanged.

## Verification

Add a regression test with a backend whose remote payload is semantically
identical to the local database. The cycle must report the snapshot counts
while also reporting that no write occurred.
