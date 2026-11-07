#!/usr/bin/env bash
# Notarize a signed .dmg with Apple and staple the ticket to it, so Gatekeeper accepts it on a clean Mac, even offline.
#
#   NOTARY_KEY_PATH=... NOTARY_KEY_ID=... NOTARY_ISSUER_ID=... bash native/scripts/notarize.sh makit-0.2.7-universal.dmg
#
# Auth is an App Store Connect API key (a .p8 file): no Apple ID password, nothing interactive. The .dmg must already be
# signed with a Developer ID Application certificate and the .app inside with the hardened runtime (bundle.sh does both
# when SIGN_IDENTITY is set). Notarizing the .dmg covers the .app inside it.
set -euo pipefail

dmg="${1:?usage: notarize.sh <file.dmg>}"
for v in NOTARY_KEY_PATH NOTARY_KEY_ID NOTARY_ISSUER_ID; do
    [[ -n "${!v:-}" ]] || { echo "notarize.sh: $v is not set" >&2; exit 1; }
done
[[ -f "$NOTARY_KEY_PATH" ]] || { echo "notarize.sh: no key file at \$NOTARY_KEY_PATH" >&2; exit 1; }
auth=(--key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")

out="$(mktemp)"
trap 'rm -f "$out"' EXIT
# --wait blocks until Apple answers (usually a few minutes). A non-zero exit here can also just mean a network blip,
# so read the verdict from the JSON instead of trusting the exit code alone
xcrun notarytool submit "$dmg" "${auth[@]}" --wait --output-format json >"$out" || true
cat "$out"
status="$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("status",""))' "$out" 2>/dev/null || true)"
id="$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("id",""))' "$out" 2>/dev/null || true)"
if [[ "$status" != "Accepted" ]]; then
    echo "notarize.sh: notarization of $dmg ended as '${status:-unknown}'" >&2
    [[ -n "$id" ]] && xcrun notarytool log "$id" "${auth[@]}" >&2 || true
    exit 1
fi

xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
echo "notarized and stapled: $dmg"
