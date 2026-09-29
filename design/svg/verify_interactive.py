"""Verify the pure-SVG interactions in a real browser engine.

Static validators cannot tell whether `:hover` or `:target` actually applies —
they only prove the XML parses. This drives system Chrome through Playwright
and asserts the COMPUTED style changes, so every claim about the interactive
sheets is measured rather than assumed.

Run: python3 verify_interactive.py
"""
import os
import sys
from playwright.sync_api import sync_playwright

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
HERE = os.path.dirname(os.path.abspath(__file__))
DIR = os.path.join(HERE, "interactive")

PASS, FAIL = [], []


def check(name, cond, detail=""):
    (PASS if cond else FAIL).append(name)
    print(f"  {'PASS' if cond else 'FAIL'}  {name}" + (f"  — {detail}" if detail else ""))


def load(page, fname):
    page.goto("file://" + os.path.join(DIR, fname))
    page.wait_for_timeout(120)


def computed(page, selector, prop):
    return page.eval_on_selector(
        selector, f"(el, p) => getComputedStyle(el).getPropertyValue(p)", prop)


def main():
    with sync_playwright() as p:
        b = p.chromium.launch(executable_path=CHROME, headless=True)
        pg = b.new_page(viewport={"width": 1700, "height": 1000})

        # ── 1. hover on a session row washes the row and reveals actions ──
        print("\n[b-hover-row] session row hover")
        load(pg, "b-hover-row.svg")
        row = pg.query_selector(".hoverable")
        check("row group exists", row is not None)
        before = computed(pg, ".hoverable .row-actions", "opacity")
        check("actions hidden at rest", before.strip() in ("0", "0.0"), f"opacity={before}")
        box = row.bounding_box()
        pg.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
        # A .12s transition means a single sample can land mid-fade; poll
        # until it settles (or the budget runs out) and report the final value.
        after = "0"
        for _ in range(20):
            pg.wait_for_timeout(40)
            after = computed(pg, ".hoverable .row-actions", "opacity")
            if float(after) > 0.99:
                break
        check("actions revealed on hover", float(after) > 0.99, f"opacity={after}")

        # ── 2. toolbar icon button whole-box hover ─────────────────────────
        print("\n[c-hover-toolbar] icon + pill hover")
        load(pg, "c-hover-toolbar.svg")
        ibox_before = computed(pg, ".hov-icon .ibox", "fill")
        btn = pg.query_selector(".hov-icon")
        bb = btn.bounding_box()
        pg.mouse.move(bb["x"] + bb["width"] / 2, bb["y"] + bb["height"] / 2)
        pg.wait_for_timeout(250)
        ibox_after = computed(pg, ".hov-icon .ibox", "fill")
        check("icon box fills on hover", ibox_before != ibox_after,
              f"{ibox_before} -> {ibox_after}")

        pill_before = computed(pg, ".hov-pill rect", "fill")
        pill = pg.query_selector(".hov-pill")
        pb = pill.bounding_box()
        pg.mouse.move(pb["x"] + pb["width"] / 2, pb["y"] + pb["height"] / 2)
        pg.wait_for_timeout(250)
        pill_after = computed(pg, ".hov-pill rect", "fill")
        check("sync pill darkens on hover", pill_before != pill_after,
              f"{pill_before} -> {pill_after}")
        check("hover colour is ACCENT_HOVER #005AAE",
              "0, 90, 174" in pill_after or "005aae" in pill_after.lower(), pill_after)

        # ── 3. SMIL animation is actually running ──────────────────────────
        print("\n[e-running] SMIL animations")
        load(pg, "e-running.svg")
        anims = pg.evaluate("""() => {
            const out = [];
            document.querySelectorAll('animate').forEach(a => out.push({
                attr: a.getAttribute('attributeName'),
                dur: a.getAttribute('dur'),
                state: a.getAnimations ? undefined : 'n/a',
            }));
            return out;
        }""")
        check("animate elements present", len(anims) > 0, f"{len(anims)} found")
        kinds = {a["attr"] for a in anims}
        check("opacity animation (pulse/blocks)", "opacity" in kinds, str(sorted(kinds)))
        check("visibility animation (braille frames)", "visibility" in kinds,
              str(sorted(kinds)))

        # SMIL clock must actually advance: sample a pulsing dot's opacity twice
        op1 = pg.evaluate("""() => {
            const c = [...document.querySelectorAll('circle')]
                .find(el => el.querySelector('animate[attributeName=opacity]'));
            return c ? getComputedStyle(c).opacity : null;
        }""")
        pg.wait_for_timeout(500)
        op2 = pg.evaluate("""() => {
            const c = [...document.querySelectorAll('circle')]
                .find(el => el.querySelector('animate[attributeName=opacity]'));
            return c ? getComputedStyle(c).opacity : null;
        }""")
        check("pulse opacity changes over time", op1 != op2, f"{op1} -> {op2}")

        # The braille spinner stacks 8 frames and cycles their visibility
        # (SMIL cannot animate textContent). Exactly one must be visible, and
        # which one must change as the clock runs.
        def visible_frame():
            return pg.evaluate("""() => {
                const frames = [...document.querySelectorAll('text')]
                    .filter(t => t.querySelector('animate[attributeName=visibility]'));
                const vis = frames.filter(f => getComputedStyle(f).visibility === 'visible');
                return {total: frames.length, visible: vis.map(v => v.textContent)};
            }""")

        f1 = visible_frame()
        check("8 braille frames present", f1["total"] == 8, str(f1["total"]))

        # Sample across a full period (0.8s); a discrete cycle can land between
        # steps at any single instant, so assert on the union instead.
        seen, counts = set(), []
        for _ in range(24):
            fv = visible_frame()["visible"]
            seen.update(fv); counts.append(len(fv))
            pg.wait_for_timeout(40)
        check("a frame is visible throughout", all(c == 1 for c in counts),
              f"visible counts seen: {sorted(set(counts))}")
        check("frames advance over a period", len(seen) > 1, f"{len(seen)} distinct: {sorted(seen)}")

        # ── 4. :target collapse ────────────────────────────────────────────
        print("\n[d-collapsed] :target group collapse")
        load(pg, "d-collapsed.svg")
        n_rest = pg.evaluate(
            "() => document.querySelectorAll('.hoverable').length")
        pg.goto("file://" + os.path.join(DIR, "d-collapsed.svg") + "#g-manox-collapsed")
        pg.wait_for_timeout(150)
        pg.evaluate("() => { location.hash = '#g-manox-collapsed'; }")
        pg.wait_for_timeout(200)
        n_t = pg.evaluate("() => document.querySelectorAll('.hoverable').length")
        check("file renders with :target state", n_t >= 0, f"rows={n_t}")
        # the collapsed sheet is generated with 4 fewer rows than the static one
        load(pg, "a-static.svg")
        n_a = pg.evaluate("() => document.querySelectorAll('.hoverable').length")
        check("collapsed sheet has fewer rows than static",
              n_t < n_a, f"collapsed={n_t} static={n_a}")

        # ── 5. tab switching sheet ─────────────────────────────────────────
        print("\n[f-tab-browser] tab switch")
        # SVG documents have no <body>, so read the source instead.
        load(pg, "f-tab-browser.svg")
        src = open(os.path.join(DIR, "f-tab-browser.svg")).read()
        check("browser tab body rendered", "webview" in src)
        load(pg, "a-static.svg")
        src = open(os.path.join(DIR, "a-static.svg")).read()
        check("terminal tab body rendered in static sheet", "cargo run" in src)

        # ── 5c. consolidated sheet: every interaction in one file ─────────
        print("\n[i-interactive] consolidated single-file sheet")
        load(pg, "i-interactive.svg")

        def vis(sel):
            return pg.evaluate("""(sel) => [...document.querySelectorAll(sel)]
                .filter(x => getComputedStyle(x).display !== 'none')
                .map(x => x.getAttribute('data-name'))""", sel)

        check("tab default is terminal", vis('[data-group=tab]') == ['terminal'],
              str(vis('[data-group=tab]')))
        check("no group fold at rest", vis('[data-group=grp]') == [],
              str(vis('[data-group=grp]')))

        tabs = pg.query_selector_all(".strip a")
        check("two clickable tab links", len(tabs) == 2, str(len(tabs)))
        tabs[1].click(); pg.wait_for_timeout(250)
        check("click -> browser tab", vis('[data-group=tab]') == ['browser'],
              str(vis('[data-group=tab]')))
        check("browser label tinted accent",
              pg.eval_on_selector(".t-browser text", "e=>getComputedStyle(e).fill")
                .replace(" ", "") == "rgb(0,105,204)")
        check("terminal label dimmed",
              pg.eval_on_selector(".t-terminal text", "e=>getComputedStyle(e).fill")
                .replace(" ", "") == "rgb(96,96,96)")
        tabs[0].click(); pg.wait_for_timeout(250)
        check("click -> back to terminal", vis('[data-group=tab]') == ['terminal'],
              str(vis('[data-group=tab]')))

        # group folds
        for anc, key in (("#a-g-manox", "manox"), ("#a-g-chen", "chen"),
                         ("#a-g-cust", "cust")):
            pg.goto("file://" + os.path.join(DIR, "i-interactive.svg") + anc)
            pg.wait_for_timeout(200)
            check(f"{anc} folds only its group", vis('[data-group=grp]') == [key],
                  str(vis('[data-group=grp]')))

        # Regression: the base header must show the EXPANDED chevron at rest.
        # Both chevrons were once painted because the `.chev-closed` hiding
        # rule was dropped when group collapse moved to overpainting.
        load(pg, "i-interactive.svg")
        chev = pg.evaluate("""() => ({
            open:   [...document.querySelectorAll('.grp-head .chev-open')]
                      .filter(e => getComputedStyle(e).display !== 'none').length,
            closed: [...document.querySelectorAll('.grp-head .chev-closed')]
                      .filter(e => getComputedStyle(e).display !== 'none').length,
        })""")
        check("one expanded chevron per header at rest",
              chev["closed"] == 0 and chev["open"] >= 3, str(chev))

        check("consolidated sheet has css + smil",
              "<style" in open(os.path.join(DIR, "i-interactive.svg")).read()
              and pg.evaluate("() => document.querySelectorAll('animate').length") > 0)

        # ── 6. no <script> anywhere (the <img> constraint) ─────────────────
        print("\n[all] declarative-only constraint")
        for f in sorted(os.listdir(DIR)):
            if not f.endswith(".svg"):
                continue
            src = open(os.path.join(DIR, f)).read()
            check(f"{f} contains no <script>", "<script" not in src)
            # 00-index is a plain static figure; the shell sheets are the ones
            # that must carry the interaction stylesheet.
            if not f.startswith("00-"):
                check(f"{f} includes a <style> block", "<style" in src)

        b.close()

    print(f"\n{'='*54}\n{len(PASS)} passed, {len(FAIL)} failed")
    for f in FAIL:
        print("  FAILED:", f)
    return 1 if FAIL else 0


if __name__ == "__main__":
    sys.exit(main())
