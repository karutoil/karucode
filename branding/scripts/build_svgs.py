#!/usr/bin/env python3
"""Generate the text-bearing karucode SVGs with the text outlined to paths.

SVG <text> renders in whatever font the viewer has, so the wordmark would fall
back to Arial on most machines. This script shapes text with HarfBuzz using the
bundled fonts in branding/fonts and writes plain <path> data instead.

Needs: fonttools, uharfbuzz (pip install fonttools uharfbuzz).
    python3 branding/scripts/build_svgs.py
"""
import pathlib

import uharfbuzz as hb
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

ROOT = pathlib.Path(__file__).resolve().parents[1]
FONTS = ROOT / "fonts"

INK, PAPER, PAPER2, MUTED = "#111213", "#eae9e5", "#b4b3ae", "#8b8d8c"
GOLD, GOLD_DEEP = "#dfb680", "#8a5d24"


class Face:
    def __init__(self, name: str):
        path = FONTS / name
        self.tt = TTFont(path)
        self.glyphs = self.tt.getGlyphSet()
        self.upem = self.tt["head"].unitsPerEm
        self.hb_font = hb.Font(hb.Face(path.read_bytes()))

    def run(self, text: str, size: float, tracking: float = 0.0):
        """Shape text. Returns (list of (glyph_name, x_offset_px), advance_px)."""
        buf = hb.Buffer()
        buf.add_str(text)
        buf.guess_segment_properties()
        hb.shape(self.hb_font, buf, {"kern": True, "liga": True})
        order = self.tt.getGlyphOrder()
        scale = size / self.upem
        x, out = 0.0, []
        for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
            out.append((order[info.codepoint], x + pos.x_offset * scale))
            x += pos.x_advance * scale + tracking
        return out, x - tracking

    def path(self, glyphs, size: float, ox: float, baseline: float) -> str:
        scale = size / self.upem
        pen = SVGPathPen(self.glyphs)
        for name, gx in glyphs:
            # font units are y-up; SVG is y-down
            tp = TransformPen(pen, (scale, 0, 0, -scale, ox + gx, baseline))
            self.glyphs[name].draw(tp)
        return pen.getCommands()


BOLD = Face("Inter-Bold.ttf")
REGULAR = Face("Inter-Regular.ttf")
MONO = Face("JetBrainsMono-Regular.ttf")


def text(face: Face, s: str, size: float, x: float, baseline: float, fill: str, tracking: float = 0.0):
    glyphs, width = face.run(s, size, tracking)
    return f'<path fill="{fill}" d="{face.path(glyphs, size, x, baseline)}"/>', width


def wordmark(size: float, x: float, baseline: float, karu: str, code: str):
    """'karu' + 'code' as two paths, tracked like the UI (-0.04em)."""
    tracking = -0.04 * size
    karu_glyphs, karu_w = BOLD.run("karu", size, tracking)
    code_glyphs, code_w = BOLD.run("code", size, tracking)
    a = f'<path fill="{karu}" d="{BOLD.path(karu_glyphs, size, x, baseline)}"/>'
    b = f'<path fill="{code}" d="{BOLD.path(code_glyphs, size, x + karu_w + tracking, baseline)}"/>'
    return a + "\n  " + b, karu_w + tracking + code_w


def mark(tx: float, ty: float, scale: float, bar: str, chevron: str, opacity: float = 1.0) -> str:
    op = f' opacity="{opacity}"' if opacity != 1 else ""
    return (
        f'<g transform="translate({tx:g} {ty:g}) scale({scale:g})" fill="none" '
        f'stroke-linecap="round" stroke-linejoin="round" stroke-width="4.5"{op}>'
        f'<path d="M14 10V38" stroke="{bar}"/><path d="M33 10L20 24L33 38" stroke="{chevron}"/></g>'
    )


def svg(w: float, h: float, body: str, defs: str = "") -> str:
    d = f"\n  <defs>{defs}</defs>" if defs else ""
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:g} {h:g}">\n'
        f"  <title>karucode</title>{d}\n  {body}\n</svg>\n"
    )


def lockup(bar: str, chevron: str, karu: str, code: str) -> str:
    # Mark occupies 48x48 at (0,4); wordmark cap height aligned to the mark's stroke span.
    words, width = wordmark(30, 44, 38.5, karu, code)
    return svg(round(44 + width + 2), 56, mark(0, 4, 1, bar, chevron) + "\n  " + words)


def banner() -> str:
    defs = (
        '<linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">'
        f'<stop offset="0" stop-color="{INK}"/><stop offset=".6" stop-color="#141516"/>'
        '<stop offset="1" stop-color="#24201a"/></linearGradient>'
    )
    words, _ = wordmark(78, 224, 184, PAPER, GOLD)
    tag, _ = text(REGULAR, "A coding agent that is one binary, not a stack.", 24, 228, 230, PAPER2)
    body = "\n  ".join([
        '<rect width="1280" height="320" fill="url(#bg)"/>',
        mark(960, -40, 8.4, PAPER, GOLD, 0.06),
        mark(84, 96, 2.6, PAPER, GOLD),
        words,
        tag,
    ])
    return svg(1280, 320, body, defs)


def social_card() -> str:
    defs = (
        '<linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">'
        f'<stop offset="0" stop-color="{INK}"/><stop offset=".55" stop-color="#141516"/>'
        '<stop offset="1" stop-color="#2a2319"/></linearGradient>'
    )
    words, _ = wordmark(104, 96, 370, PAPER, GOLD)
    tag, _ = text(REGULAR, "A coding agent that is one binary, not a stack.", 34, 100, 432, PAPER2)
    facts, _ = text(MONO, "one Rust binary · SQLite · any OpenAI-compatible endpoint", 24, 100, 536, MUTED)
    body = "\n  ".join([
        '<rect width="1280" height="640" fill="url(#bg)"/>',
        mark(860, 40, 10, PAPER, GOLD, 0.06),
        mark(96, 150, 2.4, PAPER, GOLD),
        words,
        tag,
        f'<rect x="100" y="488" width="48" height="3" rx="1.5" fill="{GOLD}"/>',
        facts,
    ])
    return svg(1280, 640, body, defs)


def main() -> None:
    out = {
        ROOT / "logo" / "lockup-dark.svg": lockup(PAPER, GOLD, PAPER, GOLD),
        ROOT / "logo" / "lockup-light.svg": lockup(INK, GOLD_DEEP, INK, GOLD_DEEP),
        ROOT / "logo" / "lockup-mono-black.svg": lockup(INK, INK, INK, INK),
        ROOT / "logo" / "lockup-mono-white.svg": lockup("#ffffff", "#ffffff", "#ffffff", "#ffffff"),
        ROOT / "social" / "banner.svg": banner(),
        ROOT / "social" / "social-card.svg": social_card(),
    }
    for path, content in out.items():
        path.write_text(content)
        print(f"{path.relative_to(ROOT.parent)}  {len(content)} bytes")


if __name__ == "__main__":
    main()
