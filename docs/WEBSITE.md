# Turning the docs into a website

The documentation tree in this repository is importable into a static site
generator **as-is**: no frontmatter anywhere, titles from each file's single H1,
relative links intact. This page records the import: the generator, the
machine-readable index, the two link rewrites, and the known rough edges — so
whoever builds the site does not have to re-derive any of it.

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

## The two link rewrites

All markdown links in `docs/` are relative and stay that way in the repository.
A website build applies exactly two rewrites:

1. **Handbook links that escape the docs root.** `docs/MCP.md`, `docs/PLUGINS.md`,
   `docs/INTERNAL-DOCS.md` and `docs/CONTRIBUTING-AGENTS.md` link to source files
   that live outside any docs tree (`](../crates/kc-mcp)`, `](../AGENTS.md)`).
   These are correct in the repository and unresolvable on a website. Rewrite
   them to the repository URL:
   `](../X)` → `](https://github.com/karutoil/karucode/blob/main/X)` — done at
   build time (a pre-build step or a mkdocs hook), not by editing the files.
2. **README repo-root links.** README.md links to `CONTRIBUTING.md`,
   `SECURITY.md` and `CHANGELOG.md` at the repository root; when README.md
   becomes the site homepage, rewrite those to the same repository URLs. (The
   other README links already point into `docs/`.)

`#fragment` anchors pass through unchanged — the repo uses GitHub-style slugs,
which mkdocs reproduces.

## Known rough edges

* `docs/internal/` contains files that are not topics in `index.json` (the
  design system, the design audits). They are hand-added in `mkdocs.yml`'s
  `Design` section.
* `docs/internal/index.json` itself is not a page; `mkdocs.yml` excludes it from
  the nav (mkdocs copies it as a static asset — harmless, and useful for anyone
  who wants the raw index).
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
