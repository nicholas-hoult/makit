#!/bin/sh
# Local/CI build for the marketing site. Does not deploy.
#
# Changelog pages are rendered from the public GitHub Releases API
# (published, not draft, not prerelease). Unreleased changelog headings
# are not shown. If that API cannot be read, this script exits non-zero
# and does not write a substitute from CHANGELOG.md.
#
# Proposed Cloudflare Pages Git settings (not applied; project access was refused):
#   Production branch: main
#   Build command: bash scripts/build-site.sh
#   Build output directory: site
#   Root directory: / (repository root)
# A non-main branch must stay a preview and must not replace production.
# Native Git integration builds on branch pushes. It does not build on a
# release tag. The unpublished workflow in docs/proposed/cloudflare-pages.yml
# is the release trigger, and it refuses a tag whose commit is not on main.
set -eu
cd "$(dirname "$0")/.."
mkdir -p site/assets
cp screenshots/dark.jpg screenshots/light.jpg screenshots/catppuccin.jpg screenshots/dayfox.jpg site/assets/
python3 scripts/extract-app-icon.py
python3 scripts/render-site-changelog.py
