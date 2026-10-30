#!/usr/bin/env python3
"""Render the karucode brand SVGs to PNG/ICO and copy web assets.

Needs: chromium on PATH, Pillow. Run from anywhere:
    python3 branding/scripts/render.py
"""
import pathlib
import re
import shutil
import subprocess
import tempfile

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[1]
PNG = ROOT / "png"
WEB_PUBLIC = ROOT.parent / "web" / "public"
CHROMIUM = shutil.which("chromium") or shutil.which("chromium-browser") or shutil.which("google-chrome")


def shoot(svg: pathlib.Path, w: int, h: int, out: pathlib.Path, scale: int = 1) -> None:
    """Screenshot an SVG at exactly w x h CSS px (times scale) with a transparent background."""
    with tempfile.TemporaryDirectory() as tmp:
        page = pathlib.Path(tmp) / "page.html"
        page.write_text(
            "<!doctype html><html><body style='margin:0;background:transparent'>"
            f"<img src='{svg.as_uri()}' style='display:block;width:{w}px;height:{h}px'>"
            "</body></html>"
        )
        subprocess.run(
            [CHROMIUM, "--headless", "--disable-gpu", "--no-sandbox", "--hide-scrollbars",
             "--default-background-color=00000000", f"--force-device-scale-factor={scale}",
             f"--window-size={w},{h}", f"--screenshot={out}", page.as_uri()],
            check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60,
        )


def downscale(master: pathlib.Path, sizes, name: str) -> None:
    img = Image.open(master).convert("RGBA")
    for s in sizes:
        img.resize((s, s), Image.LANCZOS).save(PNG / name.format(s))


def main() -> None:
    if not CHROMIUM:
        raise SystemExit("chromium not found on PATH")
    PNG.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp)
        jobs = {
            "app": ROOT / "icons" / "app-icon.svg",
            "fav": ROOT / "icons" / "favicon.svg",
            "mask": ROOT / "icons" / "app-icon-maskable.svg",
            "mark": ROOT / "logo" / "mark.svg",
            "mark-light": ROOT / "logo" / "mark-on-light.svg",
        }
        masters = {}
        for key, svg in jobs.items():
            masters[key] = tmp / f"{key}.png"
            shoot(svg, 1024, 1024, masters[key])

        downscale(masters["app"], [1024, 512, 256, 192, 180, 128, 64], "app-icon-{}.png")
        downscale(masters["fav"], [48, 32, 16], "favicon-{}.png")
        downscale(masters["mask"], [512, 192], "app-icon-maskable-{}.png")
        downscale(masters["mark"], [1024, 512], "mark-{}.png")
        downscale(masters["mark-light"], [1024, 512], "mark-on-light-{}.png")

        fav = Image.open(masters["fav"]).convert("RGBA")
        fav.save(PNG / "favicon.ico", sizes=[(16, 16), (32, 32), (48, 48)])

    for name in ("lockup-dark", "lockup-light", "lockup-mono-black", "lockup-mono-white"):
        svg = ROOT / "logo" / f"{name}.svg"
        vb = re.search(r'viewBox="0 0 ([\d.]+) ([\d.]+)"', svg.read_text())
        w, h = float(vb.group(1)), float(vb.group(2))
        shoot(svg, round(w * 224 / h), 224, PNG / f"{name}.png")
    # Marketing images live beside their SVG so they can be linked from the README.
    shoot(ROOT / "social" / "banner.svg", 1280, 320, ROOT / "social" / "banner.png", scale=2)
    shoot(ROOT / "social" / "social-card.svg", 1280, 640, ROOT / "social" / "social-card.png")

    # Web UI assets (copied into web/dist and embedded in the binary by the build).
    shutil.copyfile(ROOT / "icons" / "favicon.svg", WEB_PUBLIC / "favicon.svg")
    shutil.copyfile(PNG / "favicon.ico", WEB_PUBLIC / "favicon.ico")
    shutil.copyfile(PNG / "app-icon-180.png", WEB_PUBLIC / "apple-touch-icon.png")

    for p in sorted(PNG.iterdir()) + sorted((ROOT / "social").glob("*.png")):
        with Image.open(p) as im:
            print(f"{p.relative_to(ROOT.parent)}  {im.size[0]}x{im.size[1]}")


if __name__ == "__main__":
    main()
