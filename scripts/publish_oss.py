#!/usr/bin/env python3
"""Publish Clippi installers and the auto-update manifest to Aliyun OSS.

Called by the release workflow after all build artifacts are available. Each
platform installer is uploaded twice:

  * ``<prefix>/v<version>/<stable-name>`` — immutable, long-lived object the
    update manifest points at (no check/download race with newer releases).
  * ``<prefix>/<stable-name>``           — stable alias used by the website
    download buttons.

``<prefix>/latest.json`` is uploaded last so the desktop client can only ever
observe a complete release. It contains the version, release notes and, for
every platform, the object name/path, size and SHA256 the client verifies
before installing.

The stable index is **monotonic**: before touching the stable aliases or
``latest.json``, the publisher reads the currently published manifest and only
advances it when the incoming version is the same or newer. Backfilling an
older release therefore uploads its versioned artifacts without rolling the
website/update index backwards. The read/compare/write sequence of the latest
pointers is guarded by a ``publish.lock`` object.

OSS has no reliable compare-and-delete, so the lock is a **secondary** guard:
the primary cross-workflow mutual exclusion is the GitHub Actions
``clippi-oss-publish`` concurrency group shared by ``release.yml`` and
``publish-oss.yml``. Stale locks are never reclaimed automatically — a crashed
run leaves the lock in place and the next run fails with instructions to delete
it manually. Release verifies the lock owner/ETag before deleting it.

Versioned artifacts are **immutable**: they are uploaded with
``x-oss-forbid-overwrite`` and, when an object already exists, its remote
SHA256 is compared with the local file. A mismatch aborts the publish instead of
silently replacing bytes that a published manifest may already reference — bump
the version instead. This also prevents two concurrent publishers of the same
version from racing on ``v{version}/...`` while each computes its own manifest
hash: one upload wins, the other either verifies identical bytes or fails
without touching the stable pointers.

Only stable releases are published here: prerelease tags (``1.2.3-beta.1``)
are rejected even through the manual workflow, because the OSS channel is the
stable update index.

The publishing credentials need ``PutObject``/``GetObject``/``DeleteObject``
and ``GetObjectAcl``/``PutObjectAcl`` on the release prefix (the lock object is
deleted after each run).

Usage:
  OSS_ACCESS_KEY_ID=... OSS_ACCESS_KEY_SECRET=... \
    python3 scripts/publish_oss.py --artifacts artifacts --tag v0.4.7 \
      --notes-file release-notes.md

  python3 scripts/publish_oss.py --artifacts artifacts --tag v0.4.7 --dry-run
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path

DEFAULT_ENDPOINT = "oss-cn-shanghai.aliyuncs.com"
DEFAULT_BUCKET = "rains-ailurus-cn"
DEFAULT_PREFIX = "clippi/releases"
MANIFEST_SCHEMA = 1
MANIFEST_NAME = "latest.json"
LOCK_NAME = "publish.lock"

# (artifact glob, stable object name, manifest platform key, content type)
UPLOADS = [
    (
        "Clippi_*_x64-setup.exe",
        "Clippi_Setup.exe",
        "windows-x86_64",
        "application/octet-stream",
    ),
    (
        "Clippi_aarch64.dmg",
        "Clippi_aarch64.dmg",
        "macos-aarch64",
        "application/x-apple-diskimage",
    ),
    (
        "Clippi_x86_64.dmg",
        "Clippi_x64.dmg",
        "macos-x86_64",
        "application/x-apple-diskimage",
    ),
]

SEMVER_RE = re.compile(
    r"^(?P<major>\d+)\.(?P<minor>\d+)\.(?P<patch>\d+)"
    r"(?:-(?P<prerelease>[0-9A-Za-z.-]+))?"
    r"(?:\+(?P<build>[0-9A-Za-z.-]+))?$"
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--artifacts",
        required=True,
        help="directory containing the downloaded build artifacts",
    )
    parser.add_argument("--tag", required=True, help="release tag, e.g. v0.4.7")
    parser.add_argument(
        "--notes-file",
        help="markdown release notes embedded in the manifest",
    )
    parser.add_argument(
        "--endpoint", default=os.environ.get("OSS_ENDPOINT", DEFAULT_ENDPOINT)
    )
    parser.add_argument("--bucket", default=os.environ.get("OSS_BUCKET", DEFAULT_BUCKET))
    parser.add_argument("--prefix", default=os.environ.get("OSS_PREFIX", DEFAULT_PREFIX))
    parser.add_argument(
        "--access-key-id", default=os.environ.get("OSS_ACCESS_KEY_ID", "")
    )
    parser.add_argument(
        "--access-key-secret", default=os.environ.get("OSS_ACCESS_KEY_SECRET", "")
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the upload plan and manifest without touching OSS",
    )
    return parser.parse_args()


def release_version(tag: str) -> str:
    """Return the plain stable semver version from a release tag.

    Prereleases are rejected: the OSS channel is the stable update index, and
    the manual backfill workflow must not be able to push a beta to stable
    users.
    """
    version = tag.rsplit("/", 1)[-1]
    if version.startswith("v"):
        version = version[1:]
    match = SEMVER_RE.match(version)
    if not match:
        raise SystemExit(f"invalid release tag: {tag}")
    if match.group("prerelease"):
        raise SystemExit(
            f"refusing to publish prerelease {version} to the stable OSS channel"
        )
    return version


def version_core(version: str) -> tuple[int, int, int]:
    """Parse the semver precedence core (build metadata is ignored)."""
    match = SEMVER_RE.match(version.strip().lstrip("v"))
    if not match:
        raise SystemExit(f"invalid semver in published manifest: {version}")
    return (
        int(match.group("major")),
        int(match.group("minor")),
        int(match.group("patch")),
    )


def should_advance_latest(incoming: str, published: str | None) -> bool:
    """Return True when `incoming` may move the stable latest index.

    Equal versions are allowed so a failed publish can be re-run idempotently;
    older versions only upload their versioned artifacts.
    """
    if published is None:
        return True
    return version_core(incoming) >= version_core(published)


def find_artifact(artifacts: Path, pattern: str) -> Path:
    matches = sorted(artifacts.glob(pattern))
    if len(matches) != 1:
        names = ", ".join(match.name for match in matches) or "none"
        raise SystemExit(
            f"expected exactly one artifact for {pattern}, found {len(matches)}: {names}"
        )
    return matches[0]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_notes(path: str | None) -> str:
    if not path:
        return ""
    notes = Path(path).read_text(encoding="utf-8").strip()
    if not notes:
        raise SystemExit(f"release notes file is empty: {path}")
    return notes


def build_plan(artifacts: Path, version: str) -> list[dict]:
    plan = []
    for pattern, stable_name, platform, content_type in UPLOADS:
        source = find_artifact(artifacts, pattern)
        # The Windows build output is versioned; a mismatch means the workflow
        # downloaded the wrong release's artifacts.
        if platform == "windows-x86_64" and f"_{version}_" not in source.name:
            raise SystemExit(
                f"artifact {source.name} does not match release version {version}"
            )
        plan.append(
            {
                "source": source,
                "stable_name": stable_name,
                "platform": platform,
                "content_type": content_type,
                "size": source.stat().st_size,
                "sha256": sha256_file(source),
            }
        )
    return plan


def build_manifest(version: str, tag: str, notes: str, plan: list[dict]) -> dict:
    assets = {}
    for item in plan:
        assets[item["platform"]] = {
            "name": item["stable_name"],
            "path": f"v{version}/{item['stable_name']}",
            "size": item["size"],
            "sha256": item["sha256"],
        }
    return {
        "schema": MANIFEST_SCHEMA,
        "version": version,
        "tag": tag,
        "published_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "notes": notes,
        "assets": assets,
    }


def object_headers(content_type: str, cache_control: str) -> dict:
    return {
        "Content-Type": content_type,
        "Cache-Control": cache_control,
        "x-oss-object-acl": "public-read",
    }


def verify_acl(bucket, key: str) -> None:
    import oss2

    acl = bucket.get_object_acl(key).acl
    if acl != oss2.OBJECT_ACL_PUBLIC_READ:
        raise SystemExit(f"unexpected ACL for {key}: {acl}")


def upload_versioned_artifact(
    bucket, key: str, source: Path, content_type: str, expected_sha256: str
) -> None:
    """Upload an immutable versioned artifact, verifying any existing object.

    A version identifies a build: if the object already exists, its bytes must
    match the local file. Replacing it would invalidate a manifest hash that
    another publisher may already have published.
    """
    import oss2

    for attempt in range(2):
        try:
            bucket.put_object_from_file(
                key,
                str(source),
                headers={
                    **object_headers(
                        content_type, "public, max-age=31536000, immutable"
                    ),
                    "x-oss-forbid-overwrite": "true",
                },
            )
            verify_acl(bucket, key)
            return
        except oss2.exceptions.OssError as exc:
            if exc.status != 409:
                raise

        # The object already exists (possibly uploaded by a concurrent run):
        # verify its content instead of replacing it.
        try:
            remote = bucket.get_object(key)
        except oss2.exceptions.OssError as exc:
            if exc.status == 404 and attempt == 0:
                continue  # released between the put and the get; retry
            raise
        digest = hashlib.sha256()
        for chunk in iter(lambda: remote.read(1024 * 1024), b""):
            digest.update(chunk)
        if digest.hexdigest() != expected_sha256:
            raise SystemExit(
                f"versioned artifact {key} already exists with a different SHA256; "
                "bump the version instead of replacing it"
            )
        # Keep the existing object publicly readable and report the reuse.
        bucket.put_object_acl(key, oss2.OBJECT_ACL_PUBLIC_READ)
        verify_acl(bucket, key)
        print(f"Existing {key} matches the local artifact; keeping it")
        return

    raise SystemExit(f"could not publish versioned artifact {key}")


def read_published_manifest(bucket, key: str) -> dict | None:
    """Return the currently published manifest, or None when it is absent.

    An unreadable/corrupt manifest is treated as absent so the next publisher
    can replace it with a valid one (and is reported as a warning).
    """
    import oss2

    try:
        raw = bucket.get_object(key).read()
    except oss2.exceptions.OssError as exc:
        if exc.status == 404:
            return None
        raise
    try:
        manifest = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        print(f"::warning::ignoring unreadable published manifest {key}: {exc}")
        return None
    if not isinstance(manifest, dict) or not isinstance(manifest.get("version"), str):
        print(f"::warning::ignoring published manifest without a version: {key}")
        return None
    return manifest


def read_lock_payload(bucket, key: str) -> dict | None:
    """Return the lock payload, or None when it is absent/unreadable."""
    import oss2

    try:
        raw = bucket.get_object(key).read()
    except oss2.exceptions.OssError as exc:
        if exc.status == 404:
            return None
        raise
    try:
        data = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return None
    return data if isinstance(data, dict) else None


def describe_lock(bucket, key: str) -> str:
    """Best-effort lock description for error messages."""
    data = read_lock_payload(bucket, key)
    if data is None:
        return "an unreadable or already-released lock"
    return (
        f"tag={data.get('tag', 'unknown')}, "
        f"owner={data.get('owner', 'unknown')}, "
        f"started_at={data.get('started_at', 'unknown')}"
    )


def acquire_publish_lock(bucket, key: str, version: str, tag: str) -> tuple[str, str]:
    """Acquire the publish lock; never reclaims a lock this run does not own.

    Returns ``(owner, etag)`` so the caller can verify ownership on release.
    OSS has no compare-and-delete, so the primary mutual exclusion is the
    GitHub Actions ``clippi-oss-publish`` concurrency group shared by the
    release and backfill workflows; this object is a secondary guard (and the
    only guard for local runs).
    """
    import oss2

    owner = f"{os.environ.get('GITHUB_RUN_ID', 'local')}-{uuid.uuid4().hex}"
    payload = json.dumps(
        {
            "version": version,
            "tag": tag,
            "owner": owner,
            "started_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        },
        ensure_ascii=False,
    ).encode("utf-8")
    headers = {
        "Content-Type": "application/json; charset=utf-8",
        "x-oss-forbid-overwrite": "true",
    }

    try:
        result = bucket.put_object(key, payload, headers=headers)
    except oss2.exceptions.OssError as exc:
        if exc.status != 409:
            raise
        raise SystemExit(
            f"publish lock {key} is held by {describe_lock(bucket, key)}; "
            "wait for that run to finish, or delete the lock object manually "
            "if the run is gone"
        )
    print(f"Acquired publish lock {key} (owner {owner})")
    return owner, getattr(result, "etag", "") or ""


def release_publish_lock(bucket, key: str, owner: str, etag: str) -> None:
    """Release the lock only when it is still the one this run acquired.

    OSS has no compare-and-delete, so a read-then-delete window is unavoidable.
    The shared GitHub Actions concurrency group keeps workflow publishers from
    racing here; the owner/ETag check stops a slow run from deleting a newer
    lock in the common case.
    """
    import oss2

    try:
        head = bucket.head_object(key)
    except oss2.exceptions.OssError as exc:
        if exc.status == 404:
            return
        print(f"::warning::cannot inspect publish lock {key}: {exc}")
        return

    current_etag = getattr(head, "etag", "") or ""
    if etag and current_etag == etag:
        bucket.delete_object(key)
        print(f"Released publish lock {key}")
        return

    data = read_lock_payload(bucket, key)
    if data is not None and data.get("owner") == owner:
        bucket.delete_object(key)
        print(f"Released publish lock {key}")
        return

    print(
        f"::warning::publish lock {key} is no longer owned by this run; leaving it alone"
    )


def main() -> int:
    args = parse_args()
    version = release_version(args.tag)
    artifacts = Path(args.artifacts)
    if not artifacts.is_dir():
        raise SystemExit(f"artifacts directory not found: {artifacts}")

    prefix = args.prefix.strip().strip("/")
    if not prefix:
        raise SystemExit("OSS prefix must not be empty")

    notes = read_notes(args.notes_file)
    plan = build_plan(artifacts, version)
    manifest = build_manifest(version, args.tag, notes, plan)
    manifest_bytes = (
        json.dumps(manifest, ensure_ascii=False, indent=2).encode("utf-8") + b"\n"
    )

    if args.dry_run:
        for item in plan:
            print(
                f"[dry-run] {item['source']} -> {prefix}/v{version}/{item['stable_name']} "
                f"({item['size']} bytes, sha256 {item['sha256']})"
            )
            print(f"[dry-run] {item['source']} -> {prefix}/{item['stable_name']} (stable alias)")
        print(f"[dry-run] manifest -> {prefix}/{MANIFEST_NAME}")
        print(manifest_bytes.decode("utf-8"))
        return 0

    access_key_id = args.access_key_id.strip()
    access_key_secret = args.access_key_secret.strip()
    if not access_key_id or not access_key_secret:
        raise SystemExit("Missing OSS_ACCESS_KEY_ID or OSS_ACCESS_KEY_SECRET")

    import oss2

    endpoint = args.endpoint.strip()
    if not endpoint.startswith(("http://", "https://")):
        endpoint = "https://" + endpoint
    bucket = oss2.Bucket(
        oss2.Auth(access_key_id, access_key_secret),
        endpoint,
        args.bucket.strip(),
    )

    # 1. Versioned artifacts first: the manifest must never point at a missing
    #    object. They are immutable, so concurrent same-version publishers
    #    cannot leave a manifest hash that does not match the stored bytes.
    for item in plan:
        key = f"{prefix}/v{version}/{item['stable_name']}"
        print(f"Publishing {item['source']} -> oss://{args.bucket}/{key}")
        upload_versioned_artifact(
            bucket,
            key,
            item["source"],
            item["content_type"],
            item["sha256"],
        )

    # 2. Serialize the "latest" pointer update. The lock is held only while
    #    reading the published version, comparing and updating the pointers.
    manifest_key = f"{prefix}/{MANIFEST_NAME}"
    lock_key = f"{prefix}/{LOCK_NAME}"
    published_version = None
    advanced = False
    lock_owner, lock_etag = acquire_publish_lock(bucket, lock_key, version, args.tag)
    try:
        published = read_published_manifest(bucket, manifest_key)
        published_version = published["version"] if published else None
        if not should_advance_latest(version, published_version):
            print(
                f"Skipping latest pointers: {version} is older than published "
                f"{published_version}; stable aliases and {MANIFEST_NAME} left untouched"
            )
        else:
            # 3. Stable aliases used by the official website download buttons.
            for item in plan:
                key = f"{prefix}/{item['stable_name']}"
                print(f"Uploading {item['source']} -> oss://{args.bucket}/{key}")
                bucket.put_object_from_file(
                    key,
                    str(item["source"]),
                    headers=object_headers(item["content_type"], "no-cache"),
                )
                verify_acl(bucket, key)

            # 4. Manifest last, so clients only ever see a complete release.
            print(f"Uploading manifest -> oss://{args.bucket}/{manifest_key}")
            bucket.put_object(
                manifest_key,
                manifest_bytes,
                headers=object_headers(
                    "application/json; charset=utf-8",
                    "no-cache, no-store, must-revalidate",
                ),
            )
            verify_acl(bucket, manifest_key)
            remote = bucket.get_object(manifest_key).read()
            if remote != manifest_bytes:
                raise SystemExit("uploaded manifest does not match the local manifest")
            advanced = True
    finally:
        release_publish_lock(bucket, lock_key, lock_owner, lock_etag)

    published = ", ".join(item["stable_name"] for item in plan)
    if advanced:
        print(f"Published Clippi {version}: {published} + {MANIFEST_NAME}")
    else:
        print(
            f"Uploaded versioned artifacts for Clippi {version}; "
            f"latest index kept at {published_version}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
