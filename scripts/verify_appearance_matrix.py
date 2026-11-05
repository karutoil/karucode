#!/usr/bin/env python3
"""Exercise every Theme Studio / Settings appearance axis in real Chromium.

This complements component tests: Preact signals, CSS cascade, focus wrapping and
intrinsic flex/grid overflow only meet here. It does not start or change a server.
Point --base at an already-served production build (for example Vite preview).
Screenshots and a JSON report are written only under docs/internal/design-verify.
"""
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from design_verify import OUT, Session, SET_THEME, free_port, wait_for

ROOT = Path(__file__).resolve().parent.parent
PASS: list[str] = []
FAIL: list[str] = []
FINDINGS: dict[str, object] = {}

OPEN_STUDIO = """
const button = document.querySelector('.theme-header-btn');
if (!button) return 'missing opener';
button.click();
for (let i = 0; i < 60; i++) {
  if (document.querySelector('.theme-studio')) return 'open';
  await new Promise(r => setTimeout(r, 50));
}
return 'timeout';
"""

# Scope the shared, composited contrast maths to the actual drawer. Measuring the
# background workspace too would count ink covered by a scrim as visible text.
STUDIO_MEASURE = """
const drawer = document.querySelector('.theme-studio');
if (!drawer) return { total: 0, failCount: 0, fails: [{text:'drawer missing', ratio:0, px:0, cls:''}] };
const srgb = c => { c /= 255; return c <= .03928 ? c / 12.92 : Math.pow((c + .055) / 1.055, 2.4); };
const lum = c => .2126 * srgb(c.r) + .7152 * srgb(c.g) + .0722 * srgb(c.b);
const parse = s => { const m = s.match(/rgba?\\(([^)]+)\\)/); if (!m) return null;
  const p = m[1].split(/[,\\s/]+/).filter(Boolean).map(Number);
  return {r:p[0],g:p[1],b:p[2],a:p.length>3?p[3]:1}; };
const background = el => {
  const layers=[]; for(let n=el;n;n=n.parentElement){const c=parse(getComputedStyle(n).backgroundColor);
    if(c&&c.a>0){layers.push(c);if(c.a>=.999)break;}}
  if(!layers.length)return null;
  let out=layers[layers.length-1];
  for(let i=layers.length-2;i>=0;i--){const t=layers[i],a=t.a;
    out={r:t.r*a+out.r*(1-a),g:t.g*a+out.g*(1-a),b:t.b*a+out.b*(1-a)};}
  return out;
};
const ratio=(a,b)=>(Math.max(lum(a),lum(b))+.05)/(Math.min(lum(a),lum(b))+.05);
const all=[],fails=[];
drawer.querySelectorAll('*').forEach(el=>{
  const text=[...el.childNodes].filter(n=>n.nodeType===3).map(n=>n.textContent.trim()).join(' ').trim();
  if(!text)return;
  const cs=getComputedStyle(el),r=el.getBoundingClientRect();
  if(cs.display==='none'||cs.visibility==='hidden'||Number(cs.opacity)===0||r.width<2||r.height<2)return;
  if(el.closest(':disabled,[aria-disabled=true]'))return;
  const fg=parse(cs.color),bg=background(el);if(!fg||!bg)return;
  const value=Math.round(ratio(fg,bg)*100)/100;
  const row={text:text.slice(0,60),ratio:value,px:parseFloat(cs.fontSize),cls:String(el.className).slice(0,80)};
  all.push(row);if(value<4.5)fails.push(row);
});
fails.sort((a,b)=>a.ratio-b.ratio);
return {total:all.length,failCount:fails.length,fails:fails.slice(0,15)};
"""

DRAWER_STATE = """
const root = document.documentElement;
const drawer = document.querySelector('.theme-studio');
if (!drawer) return {error:'drawer missing'};
const visible = el => {
  const r=el.getBoundingClientRect(),cs=getComputedStyle(el);
  return r.width>0&&r.height>0&&cs.display!=='none'&&cs.visibility!=='hidden';
};
const overflow=[];
drawer.querySelectorAll('section,.theme-setting-grid,.theme-transcript-grid,.theme-segments,.theme-range-row,.theme-studio-foot-top,.theme-foot-buttons').forEach(el=>{
  if(!visible(el)||el.scrollWidth<=el.clientWidth+2)return;
  const ox=getComputedStyle(el).overflowX;
  if(['auto','scroll'].includes(ox))return;
  overflow.push({cls:el.className,client:el.clientWidth,scroll:el.scrollWidth,overflow:ox,text:el.innerText.slice(0,100)});
});
return {
  dataset:{...root.dataset}, overflow,
  selectedTheme:drawer.querySelector('.theme-preset[aria-pressed=true]')?.dataset.testid,
  colourInputs:drawer.querySelectorAll('input[type=color]').length,
  selected:Object.fromEntries(['tool-style','grouping','thinking'].flatMap(axis=>
    [...drawer.querySelectorAll(`[data-testid^=theme-${axis}-]`)].filter(b=>b.getAttribute('aria-pressed')==='true')
      .map(b=>[axis,b.dataset.testid.replace(`theme-${axis}-`,'')]))),
  controls:Object.fromEntries(['density','font','mono','radius'].map(axis=>[axis,drawer.querySelector(`[data-testid=theme-${axis}]`)?.value])),
  mono:getComputedStyle(root).getPropertyValue('--font-mono').trim(),
  ui:getComputedStyle(document.body).fontFamily,
  radius:getComputedStyle(root).getPropertyValue('--radius').trim(),
  docWidth:root.scrollWidth, viewport:innerWidth,
  modalCount:document.querySelectorAll('[aria-modal=true]').length,
  transcript:{
    cards:document.querySelectorAll('.transcript .tool-card').length,
    rows:document.querySelectorAll('.transcript .tool-row').length,
    groups:document.querySelectorAll('.transcript .tool-group').length,
    folds:document.querySelectorAll('.transcript .process-fold').length,
    thoughts:document.querySelectorAll('.transcript .thinking').length,
  },
};
"""

SET_CONTROL = """
const args = %s;
for(const [axis,value] of Object.entries(args)){
  const testid = value === '' ? `theme-${axis}` : `theme-${axis}-${value}`;
  const target=document.querySelector(`[data-testid=${testid}]`);
  if(target){target.click();continue;}
  const control=document.querySelector(`[data-testid=theme-${axis}]`);
  if(!control)return `missing ${axis}`;
  control.value=String(value);
  control.dispatchEvent(new Event(control.type==='range'?'input':'change',{bubbles:true}));
}
await new Promise(r=>setTimeout(r,100));
// Custom-property inheritance and lazy transcript work settle at paint, not when
// the button's onclick returns. Measure a settled frame, including under load.
await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
return (() => {%s})();
"""


def check(name: str, ok: bool, detail: object = "") -> None:
    PASS.append(name) if ok else FAIL.append(name)
    print(f"  {'ok  ' if ok else 'FAIL'} {name}{f' — {detail}' if detail else ''}", flush=True)


def set_controls(session: Session, width: int, **values: object) -> dict:
    session._send("Emulation.setDeviceMetricsOverride", {
        "width": width, "height": 844 if width <= 620 else 1080,
        "deviceScaleFactor": 1, "mobile": width <= 620,
    })
    result = session.evaluate(SET_CONTROL % (json.dumps(values), DRAWER_STATE))
    if not isinstance(result, dict):
        raise RuntimeError(f"could not set {values}: {result}")
    return result


def shot(session: Session, name: str) -> None:
    directory = OUT / "screens"
    directory.mkdir(parents=True, exist_ok=True)
    session.screenshot(directory / f"matrix-{name}.png")


def audit(session: Session, name: str, state: dict, *, contrast: bool = False, capture: bool = False) -> dict:
    geometry_bad = bool(state.get("error")) or state.get("docWidth") != state.get("viewport") or bool(state.get("overflow"))
    count_bad = state.get("colourInputs") != 18
    FINDINGS[name] = state
    result = None
    bad = geometry_bad or count_bad
    if contrast:
        result = session.evaluate(STUDIO_MEASURE)
        FINDINGS[f"{name}:contrast"] = result
        bad = bad or result.get("failCount", 1) != 0
    if bad or capture:
        shot(session, name)
    if result:
        check(f"{name}: contrast", result.get("failCount") == 0, f"{result.get('failCount')} / {result.get('total')}")
    check(f"{name}: controls fit", not geometry_bad, state.get("overflow") or "ok")
    check(f"{name}: all 18 colours", not count_bad, state.get("colourInputs"))
    return state


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default="http://127.0.0.1:4173")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()
    tmp = Path(tempfile.mkdtemp(prefix="kc-appearance-matrix-"))
    port = free_port()
    profile = tmp / "profile"
    profile.mkdir()
    log = open(tmp / "chrome.log", "wb")
    chrome = subprocess.Popen([
        "chromium", "--headless=new", "--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage",
        "--hide-scrollbars", f"--remote-debugging-port={port}", f"--user-data-dir={tmp / 'profile'}",
        "--window-size=1920,1080", "about:blank",
    ], stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    profile.mkdir(parents=True, exist_ok=True)
    try:
        wait_for(f"http://127.0.0.1:{port}/json/version")
        import urllib.request
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list") as response:
            targets = json.loads(response.read())
        session = Session(next(t for t in targets if t.get("type") == "page")["webSocketDebuggerUrl"])
        session._send("Page.enable")
        session._send("Runtime.enable")

        def size(width: int, height: int = 900, mobile: bool = False) -> None:
            session._send("Emulation.setDeviceMetricsOverride", {
                "width": width, "height": height, "deviceScaleFactor": 1, "mobile": mobile,
            })

        size(1920, 1080)
        session.navigate(args.base)
        session.evaluate(SET_THEME % ("ember", "ember", "'ember'"))
        session.navigate(args.base)
        check("Theme Studio opens", session.evaluate(OPEN_STUDIO) == "open")
        state = session.evaluate(DRAWER_STATE)
        audit(session, "ember-default", state, contrast=True, capture=True)
        check(
            "Focus wraps from last visible control",
            session.evaluate("""
              const last=document.querySelector('[data-testid=theme-save-as-new]');last.focus();
              const event=new KeyboardEvent('keydown',{key:'Tab',bubbles:true,cancelable:true});
              document.dispatchEvent(event);
              return {prevented:event.defaultPrevented,active:document.activeElement?.getAttribute('aria-label')};
            """)["prevented"],
        )

        for theme in ("ember", "nocturne", "forest", "sakura", "graphite", "daybreak", "paper", "mint"):
            state = set_controls(session, 1920, **{f"preset-{theme}": ""})
            expected_mode = "light" if theme in {"daybreak", "paper", "mint"} else "dark"
            check(f"theme/{theme}: applied", state["dataset"].get("mode") == expected_mode and state["selectedTheme"] == f"theme-preset-{theme}")
            audit(session, f"theme-{theme}", state, contrast=True, capture=theme in {"ember", "daybreak"})

        # Reset to one built-in before exhaustive transcript combinations.
        set_controls(session, 1920, **{"preset-ember": ""})
        for tool in ("cards", "rows"):
            for grouping in ("none", "tools", "turns"):
                for thinking in ("collapsed", "preview", "expanded"):
                    state = set_controls(session, 1920, **{
                        "tool-style": tool, "grouping": grouping, "thinking": thinking,
                    })
                    name = f"transcript-{tool}-{grouping}-{thinking}"
                    check(f"{name}: applied", state["dataset"].get("toolStyle") == tool and state["dataset"].get("grouping") == grouping and state["dataset"].get("thinking") == thinking)
                    transcript = state.get("transcript", {})
                    painted = transcript.get("groups", 0) + transcript.get("rows" if tool == "rows" else "cards", 0)
                    grouping_ok = grouping == "none" or transcript.get("groups", 0) > 0
                    # The real session supplies the tools; the component suite owns
                    # settled/streaming/pending-state assertions. Here we prove the
                    # clicked option changes actual transcript DOM, not just data-*.
                    check(f"{name}: transcript paints", painted > 0 and grouping_ok, transcript)
                    audit(session, name, state, contrast=grouping == "none" and thinking == "collapsed")

        sizes = {"compact": "12px", "comfortable": "13px", "spacious": "13.25px"}
        for density, body in sizes.items():
            state = set_controls(session, 390, density=density)
            check(f"density/{density}: applied", state["dataset"].get("density") == density and state["controls"].get("density") == density)
            audit(session, f"density-{density}", state, contrast=True, capture=True)
        for font in ("system", "modern", "rounded", "humanist", "classic"):
            state = set_controls(session, 390, font=font)
            check(f"font/{font}: applied", state["controls"].get("font") == font)
            audit(session, f"font-{font}", state)
        for mono in ("default", "system", "classic"):
            state = set_controls(session, 390, mono=mono)
            check(f"mono/{mono}: applied", state["controls"].get("mono") == mono)
            audit(session, f"mono-{mono}", state)
            if mono == "default":
                check("mono/default uses bundled JetBrains Mono", "JetBrains Mono" in state["mono"], state["mono"])
        for radius in (0, 10, 22):
            state = set_controls(session, 390, radius=radius)
            check(f"radius/{radius}: applied", state["controls"].get("radius") == str(radius) and state["radius"] == f"{radius}px")
            audit(session, f"radius-{radius}", state)

        # Deliberately adverse presets stack all the likely intrinsic-size problems.
        for density in ("compact", "spacious"):
            for font in ("humanist", "classic"):
                state = set_controls(session, 320, density=density, font=font, mono="classic",
                                     radius=0 if density == "compact" else 22,
                                     **{"tool-style": "rows", "grouping": "turns", "thinking": "expanded"})
                audit(session, f"adverse-320-{density}-{font}", state, contrast=True, capture=True)

        # Appearance from light back to dark and reset must not mix a native light
        # color-scheme with the old dark palette.
        for start, target in (("ember", "light"), ("daybreak", "dark")):
            set_controls(session, 390, **{f"preset-{start}": ""})
            set_controls(session, 390, **{f"mode-{target}": ""})
            result = session.evaluate("[...document.querySelectorAll('.theme-mini-action')].find(b=>b.textContent.trim()==='Reset colours').click(); await new Promise(r=>setTimeout(r,200)); await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))); return 1;")
            state = session.evaluate(DRAWER_STATE)
            audit(session, f"reset-{start}-to-{target}", state, contrast=True, capture=True)

        OUT.mkdir(parents=True, exist_ok=True)
        (OUT / "appearance-matrix.json").write_text(json.dumps({"passed": PASS, "failed": FAIL, "findings": FINDINGS}, indent=2) + "\n")
        print(f"\n{len(PASS)} passed, {len(FAIL)} failed; {OUT / 'appearance-matrix.json'}")
        return 0 if not FAIL else 1
    finally:
        chrome.terminate()
        try:
            chrome.wait(timeout=10)
        except subprocess.TimeoutExpired:
            chrome.kill()
        log.close()
        if not args.keep:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
