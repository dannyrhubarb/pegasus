#!/usr/bin/env python3
"""Revoke stale Apple Development certificates before a cloud-signed archive.

Runs right before ios-testflight.yml's Archive step. Every archive on a
fresh CI runner mints a NEW Apple Development certificate: xcodebuild's
automatic signing (-allowProvisioningUpdates) needs a development
identity, and the previous run's private key died with its ephemeral
runner, so the old certificate can never be used again by anyone. The
account therefore accumulates one dead certificate per run until Apple's
account-wide cap stops certificate creation entirely — "Your account has
reached the maximum number of certificates. To create a new one, you must
choose a certificate to revoke." (seen live 2026-08, run 14) — at which
point every archive fails.

The durable fix is to revoke DEVELOPMENT certificates here on every run;
the archive then mints its single fresh certificate far below the cap.
Distribution certificates are never touched — the store signing identity
is cloud-managed, does not accumulate, and revoking it would be a real
event. CAVEAT: this also revokes a development certificate created by a
human's local Xcode; automatic signing silently re-creates it on their
next build-and-run, which is the accepted cost of keeping CI alive.

Best-effort by design: every failure is a workflow WARNING, never a build
failure — under the cap the archive succeeds without us, and at the cap
the archive's own error message points back here.

Env: ASC_KEY_ID, ASC_ISSUER_ID, ASC_KEY_PATH (the .p8). Needs PyJWT +
cryptography (the workflow pip-installs them).
"""
import json
import os
import time
import urllib.error
import urllib.request

import jwt  # PyJWT

API = "https://api.appstoreconnect.apple.com"
# Universal "Apple Development" plus the legacy iOS-only type.
DEV_TYPES = "DEVELOPMENT,IOS_DEVELOPMENT"


def token():
    # Short-lived ES256 JWT, minted per request batch (ASC caps exp at 20 min).
    with open(os.environ["ASC_KEY_PATH"]) as f:
        key = f.read()
    now = int(time.time())
    return jwt.encode(
        {"iss": os.environ["ASC_ISSUER_ID"], "iat": now, "exp": now + 900,
         "aud": "appstoreconnect-v1"},
        key, algorithm="ES256", headers={"kid": os.environ["ASC_KEY_ID"]})


def req(method, path):
    r = urllib.request.Request(API + path, method=method, headers={
        "Authorization": f"Bearer {token()}",
        "Content-Type": "application/json"})
    with urllib.request.urlopen(r) as resp:
        raw = resp.read()
        return json.loads(raw) if raw else None


def main():
    certs = req("GET",
                f"/v1/certificates?filter[certificateType]={DEV_TYPES}"
                "&limit=200")["data"]
    if not certs:
        print("No development certificates to revoke.")
        return
    revoked = 0
    for c in certs:
        a = c["attributes"]
        label = (f"{a.get('certificateType')} {a.get('serialNumber')} "
                 f"({a.get('displayName')}, expires {a.get('expirationDate')})")
        try:
            req("DELETE", f"/v1/certificates/{c['id']}")
            print(f"Revoked {label}")
            revoked += 1
        except urllib.error.HTTPError as e:
            detail = e.read().decode(errors="replace")[:300]
            # 403 here usually means the API key's role can't manage
            # certificates (needs Admin) — surface it loudly, don't fail.
            print(f"::warning::could not revoke {label}: {e.code} {detail}")
    print(f"Revoked {revoked}/{len(certs)} development certificates.")


if __name__ == "__main__":
    try:
        main()
    except Exception as e:  # noqa: BLE001 — cleanup must never fail the build
        print(f"::warning::certificate cleanup skipped: {e}")
