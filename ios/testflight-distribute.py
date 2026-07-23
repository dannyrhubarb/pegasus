#!/usr/bin/env python3
"""Attach the just-uploaded TestFlight build to a beta group, hands-free.

Runs as the last step of ios-testflight.yml: waits for App Store Connect
to finish processing the build (matched by CFBundleVersion == the
workflow run number), submits it to Beta App Review (no-op if a
submission already exists), and adds it to the named beta group so
testers receive it without any console clicking.

A missing group is a SOFT no-op (notice + exit 0): the group is created
once, by hand, when public testing is first set up — until then the
workflow still uploads fine.

Env: ASC_KEY_ID, ASC_ISSUER_ID, ASC_KEY_PATH (the .p8), BUNDLE_ID,
BUILD_VERSION, GROUP_NAME. Needs PyJWT + cryptography (the workflow
pip-installs them).
"""
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

import jwt  # PyJWT

API = "https://api.appstoreconnect.apple.com"
PROCESSING_TIMEOUT_SECS = 30 * 60
POLL_SECS = 60


def token():
    # Short-lived ES256 JWT, minted per request batch (ASC caps exp at 20 min).
    with open(os.environ["ASC_KEY_PATH"]) as f:
        key = f.read()
    now = int(time.time())
    return jwt.encode(
        {"iss": os.environ["ASC_ISSUER_ID"], "iat": now, "exp": now + 900,
         "aud": "appstoreconnect-v1"},
        key, algorithm="ES256", headers={"kid": os.environ["ASC_KEY_ID"]})


def req(method, path, body=None, quiet_statuses=()):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(API + path, data=data, method=method, headers={
        "Authorization": f"Bearer {token()}",
        "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(r) as resp:
            raw = resp.read()
            return resp.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        detail = e.read().decode(errors="replace")
        if e.code in quiet_statuses:
            return e.code, detail
        print(f"::error::{method} {path} -> {e.code}: {detail[:600]}")
        sys.exit(1)


def get(path):
    return req("GET", path)[1]


def main():
    bundle_id = os.environ["BUNDLE_ID"]
    version = os.environ["BUILD_VERSION"]
    group_name = os.environ["GROUP_NAME"]

    apps = get(f"/v1/apps?filter[bundleId]={urllib.parse.quote(bundle_id)}")["data"]
    if not apps:
        print(f"::error::no App Store Connect app record for {bundle_id}")
        sys.exit(1)
    app_id = apps[0]["id"]

    print(f"Waiting for build {version} of {bundle_id} to finish processing…")
    deadline = time.time() + PROCESSING_TIMEOUT_SECS
    build = None
    while time.time() < deadline:
        builds = get(f"/v1/builds?filter[app]={app_id}"
                     f"&filter[version]={urllib.parse.quote(version)}"
                     "&sort=-uploadedDate&limit=1")["data"]
        if builds:
            state = builds[0]["attributes"]["processingState"]
            if state == "VALID":
                build = builds[0]
                break
            if state in ("FAILED", "INVALID"):
                print(f"::error::build {version} processing ended in {state}")
                sys.exit(1)
            print(f"  processingState={state}, waiting…")
        else:
            print("  build not visible yet, waiting…")
        time.sleep(POLL_SECS)
    if build is None:
        print(f"::error::build {version} did not finish processing "
              f"within {PROCESSING_TIMEOUT_SECS // 60} min")
        sys.exit(1)
    build_id = build["id"]
    print(f"Build {version} processed (id {build_id}).")

    # External distribution needs a Beta App Review submission per build.
    # 409 = one already exists — fine. 422 ANOTHER_BUILD_IN_REVIEW = an
    # earlier build's review is still pending (Apple allows one per train)
    # — a normal transient state, not a pipeline failure: skip the
    # submission, still attach to the group, and the next build (or a
    # manual re-run once the pending review completes) catches up.
    status, detail = req(
        "POST", "/v1/betaAppReviewSubmissions",
        {"data": {"type": "betaAppReviewSubmissions", "relationships": {
            "build": {"data": {"type": "builds", "id": build_id}}}}},
        quiet_statuses=(409, 422))
    if status in (200, 201):
        print("Submitted for Beta App Review.")
    elif status == 409:
        print("Beta App Review submission already exists — fine.")
    elif "ANOTHER_BUILD_IN_REVIEW" in (detail or ""):
        print("::notice::an earlier build is still in Beta App Review — "
              "skipping this build's review submission (it can be submitted "
              "once the pending review completes).")
    else:
        print(f"::error::beta review submission rejected: {(detail or '')[:600]}")
        sys.exit(1)

    # Match the group by name client-side (filter[name] support varies).
    groups = [g for g in get(f"/v1/betaGroups?filter[app]={app_id}&limit=200")["data"]
              if g["attributes"]["name"] == group_name]
    if not groups:
        print(f"::notice::no beta group named '{group_name}' — build uploaded "
              "and review-submitted, but not auto-assigned. Create the group "
              "in App Store Connect (TestFlight → External Testing) and "
              "future builds will attach automatically.")
        return
    group_id = groups[0]["id"]

    status, detail = req(
        "POST", f"/v1/betaGroups/{group_id}/relationships/builds",
        {"data": [{"type": "builds", "id": build_id}]},
        quiet_statuses=(409, 422))
    if status == 204:
        print(f"Build {version} assigned to beta group '{group_name}'.")
    elif status == 409:
        print(f"Build {version} was already in beta group '{group_name}'.")
    else:
        # 422: the build isn't attachable yet (e.g. its review submission
        # was skipped above). Soft outcome — a later build supersedes it.
        print(f"::notice::could not attach build {version} to "
              f"'{group_name}' yet: {(detail or '')[:300]}")


if __name__ == "__main__":
    main()
