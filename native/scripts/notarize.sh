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
json_field() { /usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2],""))' "$1" "$2" 2>/dev/null || true; }

# Submit first without waiting, so the submission id is known even if the wait below is cut short
xcrun notarytool submit "$dmg" "${auth[@]}" --no-wait --output-format json >"$out"
cat "$out"
id="$(json_field "$out" id)"
[[ -n "$id" ]] || { echo "notarize.sh: submission failed, no id returned" >&2; exit 1; }

# Apple usually answers within minutes, but a new account's first submissions can sit in the queue for many hours.
# Give up waiting after NOTARY_TIMEOUT (the submission itself keeps processing at Apple), well before the CI job limit
timeout="${NOTARY_TIMEOUT:-3h}"
xcrun notarytool wait "$id" "${auth[@]}" --timeout "$timeout" >/dev/null || true
xcrun notarytool info "$id" "${auth[@]}" --output-format json >"$out" || true
status="$(json_field "$out" status)"

case "$status" in
    Accepted) ;;
    "In Progress")
        echo "notarize.sh: still 'In Progress' at Apple after $timeout. This is a queue delay, not a rejection." >&2
        echo "notarize.sh: submission id $id. Check later with: xcrun notarytool info $id <auth>; once it is Accepted, re-run the workflow." >&2
        exit 1 ;;
    *)
        echo "notarize.sh: notarization of $dmg ended as '${status:-unknown}' (submission $id)" >&2
        xcrun notarytool log "$id" "${auth[@]}" >&2 || true
        exit 1 ;;
esac

xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
echo "notarized and stapled: $dmg"
