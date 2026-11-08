#!/usr/bin/env python3
"""Write the marketing changelog from published GitHub Releases.

Drafts, prereleases, and the Unreleased section of CHANGELOG.md are not used.
The note text is the Release body. If the API cannot be read, this exits
non-zero and does not write the pages, so a build cannot publish a guessed list.
"""
import html
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_URL = "https://api.github.com/repos/nicholas-hoult/makit/releases?per_page=100"

def stable_releases(items):
    kept = []
    for rel in items:
        if not isinstance(rel, dict):
            raise SystemExit("GitHub Releases API returned a non-object entry")
        if rel.get("draft") or rel.get("prerelease"):
            continue
        tag = rel.get("tag_name")
        if not tag:
            raise SystemExit("a published release has no tag_name")
        kept.append(rel)
    kept.sort(key=lambda r: r.get("published_at") or "", reverse=True)
    return kept

def fetch_release_list(url):
    items = []
    seen = 0
    while url:
        seen += 1
        if seen > 10:
            raise SystemExit("GitHub Releases API returned more than 10 pages")
        req = urllib.request.Request(url, headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "makit-site-build",
            "X-GitHub-Api-Version": "2022-11-28",
        })
        try:
            with urllib.request.urlopen(req, timeout=20) as resp:
                raw = resp.read()
                link = resp.headers.get("Link") or ""
                status = resp.status
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise SystemExit(
                "GitHub Releases API unreachable (%s). "
                "The changelog pages were not written. "
                "CHANGELOG.md is not used as a fallback." % exc
            )
        if status != 200:
            raise SystemExit("GitHub Releases API HTTP %s. Changelog pages were not written." % status)
        try:
            data = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise SystemExit("GitHub Releases API returned invalid JSON (%s)." % exc)
        if isinstance(data, dict):
            raise SystemExit("GitHub Releases API error: %s" % data.get("message", "unexpected object"))
        if not isinstance(data, list):
            raise SystemExit("GitHub Releases API returned %s" % type(data).__name__)
        items.extend(data)
        url = None
        for part in link.split(","):
            if 'rel="next"' in part and "<" in part and ">" in part:
                url = part[part.find("<") + 1:part.find(">")]
    return stable_releases(items)

def safe_url(url):
    u = url.strip()
    if u.startswith(("http://", "https://", "mailto:")):
        return "javascript:" not in u.lower()
    return bool(re.match(r"^(#|\.{0,2}/)?[A-Za-z0-9_./~-]*$", u))

def render_inline(text):
    out = []
    i = 0
    while i < len(text):
        if text.startswith("**", i):
            j = text.find("**", i + 2)
            if j != -1:
                out.append("<strong>" + render_inline(text[i + 2:j]) + "</strong>")
                i = j + 2
                continue
        if text[i] == "`":
            j = text.find("`", i + 1)
            if j != -1:
                out.append("<code>" + html.escape(text[i + 1:j]) + "</code>")
                i = j + 1
                continue
        if text[i] == "[":
            m = re.match(r"\[([^\]]+)\]\(([^)\s]+)\)", text[i:])
            if m and safe_url(m.group(2)):
                out.append('<a href="%s">%s</a>' % (html.escape(m.group(2), quote=True), render_inline(m.group(1))))
                i += m.end()
                continue
        out.append(html.escape(text[i]))
        i += 1
    return "".join(out)

def render_body(body):
    lines = (body or "").splitlines()
    parts = []
    para = []
    items = []

    def flush_para():
        if para:
            parts.append("<p>" + render_inline(" ".join(para)) + "</p>")
            para.clear()

    def flush_items():
        if items:
            parts.append("<ul class=\"list\">" + "".join("<li>%s</li>" % render_inline(x) for x in items) + "</ul>")
            items.clear()

    for raw in lines:
        line = raw.rstrip()
        if not line.strip() or line.strip() == "---":
            flush_para()
            flush_items()
            continue
        if line.startswith("### "):
            flush_para()
            flush_items()
            parts.append("<h3>%s</h3>" % render_inline(line[4:]))
            continue
        if line.startswith("## "):
            flush_para()
            flush_items()
            parts.append("<h3>%s</h3>" % render_inline(line[3:]))
            continue
        if line.startswith("- "):
            flush_para()
            items.append(line[2:])
            continue
        if items and (raw.startswith("  ") or raw.startswith("\t")):
            items[-1] += " " + line.strip()
            continue
        flush_items()
        para.append(line.strip())
    flush_para()
    flush_items()
    return "\n".join(parts)

def page(lang, title, canonical, css, icon, other_href, other_label, home, back_label, releases, blurb):
    nav = []
    blocks = []
    for rel in releases:
        tag = rel["tag_name"]
        slug = re.sub(r"[^A-Za-z0-9._-]+", "-", tag)
        published = (rel.get("published_at") or "")[:10]
        label = tag if not published else "%s — %s" % (tag, published)
        nav.append('<a href="#%s">%s</a>' % (html.escape(slug), html.escape(label)))
        heading = tag if not published else "%s — %s" % (tag, published)
        url = rel.get("html_url") or ""
        link = ""
        if url.startswith("https://"):
            link = '<p class="fine"><a href="%s">%s</a></p>' % (html.escape(url, quote=True), html.escape(url))
        blocks.append('<section id="%s"><h2>%s</h2>\n%s\n%s\n</section>' % (
            html.escape(slug), html.escape(heading), link, render_body(rel.get("body") or "")))
    if not blocks:
        blocks.append("<p>%s</p>" % html.escape(blurb[1]))
    return """<!doctype html>
<html lang="%s">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>%s — makit</title>
<link rel="canonical" href="%s">
<link rel="alternate" hreflang="zh-CN" href="https://makit.unitysight.com/changelog/">
<link rel="alternate" hreflang="en" href="https://makit.unitysight.com/en/changelog/">
<link rel="alternate" hreflang="x-default" href="https://makit.unitysight.com/changelog/">
<meta property="og:image" content="https://makit.unitysight.com/assets/dark.jpg">
<link rel="icon" href="%s">
<link rel="apple-touch-icon" href="%s">
<link rel="stylesheet" href="%s">
</head>
<body>
<header class="top">
  <a class="brand" href="%s">makit</a>
  <nav>
    <a class="back" href="%s">%s</a>
    <a href="%s">%s</a>
    <a href="https://github.com/nicholas-hoult/makit">GitHub</a>
  </nav>
</header>
<div class="layout">
  <aside class="side"><h3>%s</h3>
    %s
  </aside>
  <main>
    <section id="top">
      <h1>%s</h1>
      <p class="fine">%s</p>
    </section>
    %s
  </main>
</div>
</body>
</html>
""" % (
        lang, html.escape(title), canonical, icon, icon, css,
        home, home, html.escape(back_label),
        other_href, html.escape(other_label), html.escape(title),
        "\n    ".join(nav), html.escape(title), html.escape(blurb[0]), "\n".join(blocks),
    )

def write_pages(releases):
    zh = ROOT / "site/changelog/index.html"
    en = ROOT / "site/en/changelog/index.html"
    zh.parent.mkdir(parents=True, exist_ok=True)
    en.parent.mkdir(parents=True, exist_ok=True)
    zh_blurb = (
        "正文是已发布的 GitHub Release 说明，不含草稿、预发布和未发布条目。读取失败时构建会失败，不会改用仓库里的未发布内容。",
        "没有已发布的正式版本。",
    )
    en_blurb = (
        "The text is the published GitHub Release body. Drafts, prereleases, and unreleased notes are omitted. If GitHub cannot be read, the build fails and does not substitute the repository changelog.",
        "There are no published stable releases.",
    )
    zh_html = page("zh-CN", "更新日志", "https://makit.unitysight.com/changelog/",
                   "../assets/style.css", "../assets/favicon.png", "../en/changelog/", "English", "../", "返回官网", releases, zh_blurb)
    en_html = page("en", "Changelog", "https://makit.unitysight.com/en/changelog/",
                   "../../assets/style.css", "../../assets/favicon.png", "../../changelog/", "简体中文", "../", "Back to website", releases, en_blurb)
    zh.write_text(zh_html, encoding="utf-8")
    en.write_text(en_html, encoding="utf-8")
    print("published releases", [r["tag_name"] for r in releases])

def self_check():
    sample = [
        {"tag_name": "v9.9.9", "draft": True, "prerelease": False, "published_at": "2026-01-01T00:00:00Z", "body": "draft"},
        {"tag_name": "v9.9.8", "draft": False, "prerelease": True, "published_at": "2026-01-02T00:00:00Z", "body": "pre"},
        {"tag_name": "v0.2.6", "draft": False, "prerelease": False, "published_at": "2026-11-07T09:18:11Z",
         "html_url": "https://github.com/nicholas-hoult/makit/releases/tag/v0.2.6",
         "body": "### Fixed\n- kept exact\n\n---\n\n**简体中文**\n\n### 修复\n- 保留原文\n\n**Unsigned build**: manual note"},
        {"tag_name": "v0.1.0", "draft": False, "prerelease": False, "published_at": "2026-01-01T00:00:00Z", "body": "older"},
    ]
    kept = stable_releases(sample)
    tags = [r["tag_name"] for r in kept]
    if tags != ["v0.2.6", "v0.1.0"]:
        raise SystemExit("self-check filter failed: %s" % tags)
    rendered = render_body(kept[0]["body"])
    for needle in ("kept exact", "保留原文", "Unsigned build", "manual note"):
        if needle not in rendered:
            raise SystemExit("self-check dropped release text: %s" % needle)
    if "draft" in rendered or ">pre<" in rendered:
        raise SystemExit("self-check leaked draft or prerelease")
    sample_page = page("zh-CN", "更新日志", "https://makit.unitysight.com/changelog/",
                       "../assets/style.css", "../assets/favicon.png", "../en/changelog/", "English", "../", "返回官网", [], ("说明", "空"))
    if 'class="brand" href="../">makit' not in sample_page or 'class="back" href="../">返回官网' not in sample_page:
        raise SystemExit("self-check missing Chinese home link")
    en_page = page("en", "Changelog", "https://makit.unitysight.com/en/changelog/",
                   "../../assets/style.css", "../../assets/favicon.png", "../../changelog/", "简体中文", "../", "Back to website", [], ("note", "empty"))
    if 'class="back" href="../">Back to website' not in en_page or 'href="../../">makit' in en_page:
        raise SystemExit("self-check English home link is wrong")
    print("self-check ok")

def main():
    if "--self-check" in sys.argv:
        self_check()
        return
    url = os.environ.get("MAKIT_RELEASES_URL", DEFAULT_URL)
    releases = fetch_release_list(url)
    write_pages(releases)

if __name__ == "__main__":
    main()
