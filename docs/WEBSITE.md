# Turning the docs into a website

The documentation tree in this repository is importable into a static site
generator: no frontmatter anywhere, titles from each file's single H1, and every
topic page already shaped as one page with its own sections. One step sits
between the repository and the site — [`scripts/build_site.py`](../scripts/build_site.py)
— because a repository is not a website: the homepage is the root `README.md`,
and some links point above `docs/` on purpose. This page records the import: the
generator, the staging rules, the machine-readable index, and the known rough
edges. The staging rules are tested by
[`tests/test_build_site.py`](../tests/test_build_site.py).

## Build it

```bash
python3 scripts/build_site.py
KARUCODE_SITE_DOCS=.site-docs mkdocs build --strict
```

`mkdocs.yml` reads its `docs_dir` from `KARUCODE_SITE_DOCS` (default: `docs`), so
the staging directory is the input without editing the config. The output is
`site/`; both directories are gitignored. CI runs exactly this in the `site` job,
so a broken page fails a pull request rather than the day someone publishes.

## The generator: mkdocs (material theme)

Chosen over Astro Starlight / VitePress / Docusaurus because:

1. **No frontmatter is required.** Page titles come from the first `#` heading,
   and every markdown file in this repo has exactly one H1 (the `@kc-doc`
   generator's `demote_headings()` guarantees it for `docs/internal/`; the
   handbooks were written that way). The frontmatter-first SSGs would mean
   touching ~80 files.
2. **Directory-based routing.** `docs/MCP.md` and `docs/internal/mcp.md` would
   collide as slugs in an ID-routed site; as directories they are simply
   `/mcp/` and `/internal/mcp/`.
3. **`nav:` is a plain hand-written tree**, which maps one-to-one onto
   `docs/internal/index.json` — see below.

`mkdocs.yml` at the repository root is the whole configuration. The README's
raw HTML blocks (`<p align=center>`, badges, `<img>`) render with the `attr_list`
+ `md_in_html` extensions, both enabled there.

## Staging: what `scripts/build_site.py` changes

The tree is built from **git-tracked files under `docs/`**, so a developer's site
is the one a clean clone produces. The design captures and the internal audit are
gitignored; copying `docs/` blindly would put them on one machine's site only.

On top of that tree it makes three changes, and only these three:

1. **The root pages become site pages.** `README.md` is the homepage;
   `CONTRIBUTING.md`, `SECURITY.md` and `CHANGELOG.md` are staged beside it, so
   the links the README already makes resolve. The root `SECURITY.md` replaces
   the redirect stub at `docs/SECURITY.md` (that stub exists for repository
   readers; a site should show the model itself).
2. **A link that stays inside the tree is left alone.** mkdocs resolves it, and
   CI's `--strict` build fails if it points nowhere.
3. **Everything else is rewritten, per page:**
   * a root-relative `docs/...` target loses the prefix, because the staged tree
     *is* the docs directory;
   * a target that escapes the tree (`../AGENTS.md`, `../crates/kc-mcp`) becomes
     `https://github.com/karutoil/karucode/blob/main/...`, or `tree` when it is a
     directory;
   * an image pointing outside the tree becomes a `raw.githubusercontent.com`
     URL, so the page shows the picture instead of GitHub's file view;
   * a link to a directory with a `README.md` becomes that page, because
     directories are not pages.

Fenced code blocks and inline code spans are copied verbatim: the `](../X)`
example above stays readable, and so does a shell snippet that happens to contain
a link. Anchors (`#fragment`) pass through unchanged — the repo uses GitHub-style
slugs, which mkdocs reproduces.

## The machine-readable index

`docs/internal/index.json` is written by `cargo run -p kc-docs -- --write` and
describes every generated topic:

```json
{
  "count": 33,
  "topics": [
    {
      "slug": "api-keys",
      "path": "api-keys.md",
      "title": "Issue API keys for applications",
      "summary": "Mint scoped, expiring bearer keys ...",
      "tags": ["api keys", "authentication", "..."],
      "applies_to": ["route:POST /api/auth/api-keys", "env:KARUCODE_ADMIN_TOKEN", "..."],
      "source": "crates/kc-server/src/api_keys.rs:1"
    }
  ]
}
```

That is already a nav index, a search index and a metadata index in one file:
`slug`/`path` for URLs, `title`/`summary` for the sidebar and search results,
`tags` + `applies_to` for faceting (every identifier a user might paste — env
vars, config keys, routes, tool names — is in `applies_to`). Whatever consumes
it, consume this file; do not re-scan the markdown.

`scripts/gen_docs_nav.py` turns it into the `Reference` section of `mkdocs.yml`
(`--snippet` prints just that section, `--check` verifies `mkdocs.yml` is not
stale). Run it after `kc-docs -- --write` whenever a topic is added or removed.

## Known rough edges

* `docs/internal/` contains files that are not topics in `index.json` (the
  design system, the design audits). They are hand-added in `mkdocs.yml`'s
  `Design` section.
* `docs/internal/index.json` itself is not a page; `mkdocs.yml` excludes it from
  the nav (mkdocs copies it as a static asset — harmless, and useful for anyone
  who wants the raw index). `docs/REPORT.md` is in `not_in_nav` for the same
  reason: it is linked from the README but is a verification record, not a
  handbook.
* The `@kc-doc` body convention is `#`-level headings inside each topic. On the
  site each topic renders as one page with those as its sections — the same
  shape the `karucode_docs` tool serves to the model.
* ASCII diagrams in ```` ```text ```` fences render as plain code blocks; they
  are already width-aligned, so they line up in any monospace font.

## If you want frontmatter instead

The generator writes each topic in `crates/kc-docs/src/main.rs`
(`render_doc_file()`); the same `ParsedDoc` fields that become the topic's
bullet list could be emitted as YAML frontmatter there, and a small script could
generate frontmatter for the handbooks from their first `#` line. That is only
worth doing if you move to a frontmatter-strict SSG; mkdocs makes it
unnecessary.