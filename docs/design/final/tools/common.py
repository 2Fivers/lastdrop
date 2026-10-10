"""Shared pieces for the final Lastdrop readout and flyout mocks: tokens, provider glyphs,
severity glyphs and the JS reference drawing code (pace bar, tray icon, sparkline)."""

# ---------------------------------------------------------------- provider glyphs (12 x 12 grid)
# Lastdrop's own monochrome glyphs that evoke each service. Not vendor logos.
GLYPH_PATHS = {
    "claude": '<path d="M6 1.15v9.7M1.8 3.58l8.4 4.84M1.8 8.42l8.4-4.84" stroke-width="1.35" stroke-linecap="round"/>',
    "codex": '<rect x="0.9" y="1.4" width="10.2" height="9.2" rx="2.3" stroke-width="1.05"/>'
             '<path d="M3.35 4.35 5.15 6 3.35 7.65M6.35 8.05h2.4" stroke-width="1.1" stroke-linecap="round" stroke-linejoin="round"/>',
    "copilot": '<rect x="0.9" y="2.6" width="10.2" height="7.9" rx="3" stroke-width="1.05"/>'
               '<circle cx="4.3" cy="6.55" r="1.15" fill="currentColor" stroke="none"/><circle cx="7.7" cy="6.55" r="1.15" fill="currentColor" stroke="none"/>'
               '<path d="M6 2.6V1.2" stroke-width="1.05" stroke-linecap="round"/>',
    "meshy": '<path d="M6 1.3 11.1 10.4H0.9z" stroke-width="1.05" stroke-linejoin="round"/><path d="M3.45 5.85h5.1L6 10.4z" stroke-width="1" stroke-linejoin="round"/>',
}


def glyph(name, size=12, cls="pg"):
    return (f'<svg class="{cls}" width="{size}" height="{size}" viewBox="0 0 12 12" fill="none" stroke="currentColor" '
            f'aria-hidden="true">{GLYPH_PATHS[name]}</svg>')


# ---------------------------------------------------------------- severity glyphs (WinUI InfoBar composition)
# Segoe Fluent Icons: F136 circle, F139 triangle, F13B ! (triangle), F13C ! (circle), F13E check, F13F i, F140 circle-slash
def sev(kind, size=12):
    c = "sev" if size == 12 else "sev16"
    if kind == "warn":
        return f'<span class="{c} warn" aria-hidden="true"><i>&#xF139;</i><i>&#xF13B;</i></span>'
    if kind == "crit":
        return f'<span class="{c} crit" aria-hidden="true"><i>&#xF136;</i><i>&#xF13C;</i></span>'
    if kind == "ok":
        return f'<span class="{c} ok" aria-hidden="true"><i>&#xF136;</i><i>&#xF13E;</i></span>'
    if kind == "info":
        return f'<span class="{c} info" aria-hidden="true"><i>&#xF136;</i><i>&#xF13F;</i></span>'
    if kind == "stale":
        return f'<span class="{c} stale" aria-hidden="true"><i>&#xF140;</i></span>'
    return ""


RESET = '<i class="rg" aria-hidden="true">&#xE777;</i>'

# ---------------------------------------------------------------- CSS shared by both mocks
TOKENS_CSS = r"""
/* ---------- Lastdrop tokens (values = WinUI 3 theme resources; see the specs) ---------- */
.L { --fg:#1a1a1a; --fg2:#5c5c5c; --fg3:#8a8a8a; --stale:var(--fg2); --dis:rgba(0,0,0,.361); --track:rgba(0,0,0,.447); --tick:var(--fg); --fill:var(--fg);
     --caution:#9D5D00; --critical:#C42B1C; --success:#0F7B0F; --neutral:#8A8A8A; --on:#ffffff;
     --acc:#8E3AA7; --acc-text:#722C88; --hover:rgba(0,0,0,.05);
     --card-bg:#F9F9F9; --card-stroke:rgba(0,0,0,.0578); --div:rgba(0,0,0,.0803);
     --card-shadow:0 8px 16px rgba(0,0,0,.14),0 0 2px rgba(0,0,0,.10);
     --spark:rgba(0,0,0,.447); --spark-now:var(--acc); }
.D { --fg:#ffffff; --fg2:#c5c5c5; --fg3:#9f9f9f; --stale:var(--fg2); --dis:rgba(255,255,255,.363); --track:rgba(255,255,255,.545); --tick:var(--fg); --fill:var(--fg);
     --caution:#FCE100; --critical:#FF99A4; --success:#6CCB5F; --neutral:#9D9D9D; --on:#000000;
     --acc:#D58AE8; --acc-text:#E7B3F3; --hover:rgba(255,255,255,.07);
     --card-bg:#2C2C2C; --card-stroke:rgba(0,0,0,.2); --div:rgba(255,255,255,.0837);
     --card-shadow:0 8px 16px rgba(0,0,0,.26),0 0 2px rgba(0,0,0,.4);
     --spark:rgba(255,255,255,.545); --spark-now:var(--acc); }
/* high contrast: Aquatic and Desert themes (values read from GetSysColor at runtime) */
.HA { --fg:#FFFFFF; --fg2:#FFFFFF; --fg3:#FFFFFF; --stale:#A6A6A6; --dis:#A6A6A6; --track:#FFFFFF; --tick:#FFFFFF; --fill:#FFFFFF;
      --caution:#8EE3F0; --critical:#8EE3F0; --success:#FFFFFF; --neutral:#A6A6A6; --on:#263B50;
      --acc:#8EE3F0; --acc-text:#75E9FC; --hover:transparent; --card-bg:#202020; --card-stroke:#FFFFFF; --div:#FFFFFF;
      --card-shadow:none; --spark:#FFFFFF; --spark-now:#8EE3F0; }
.HD { --fg:#3D3D3D; --fg2:#3D3D3D; --fg3:#3D3D3D; --stale:#676767; --dis:#676767; --track:#3D3D3D; --tick:#3D3D3D; --fill:#3D3D3D;
      --caution:#903909; --critical:#903909; --success:#3D3D3D; --neutral:#676767; --on:#FFF5E3;
      --acc:#903909; --acc-text:#1C5E75; --hover:transparent; --card-bg:#FFFAEF; --card-stroke:#3D3D3D; --div:#3D3D3D;
      --card-shadow:none; --spark:#3D3D3D; --spark-now:#903909; }

/* severity glyphs: two stacked Segoe Fluent Icons, as WinUI's InfoBar builds them */
.sev,.sev16{position:relative;display:inline-block;flex:none;width:12px;height:16px;vertical-align:top}
.sev i,.sev16 i{position:absolute;left:0;top:0;width:100%;font:normal 12px/16px "Segoe Fluent Icons";text-align:center}
.sev16{width:16px;height:20px} .sev16 i{font-size:16px;line-height:20px}
.sev i+i,.sev16 i+i{color:var(--on)}
.sev.warn i:first-child,.sev16.warn i:first-child{color:var(--caution)}
.sev.crit i:first-child,.sev16.crit i:first-child{color:var(--critical)}
.sev.ok i:first-child,.sev16.ok i:first-child{color:var(--success)}
.sev.info i:first-child,.sev16.info i:first-child{color:var(--acc)}
.sev.stale i,.sev16.stale i{color:var(--stale)}
.rg{font:normal 11px/16px "Segoe Fluent Icons";color:var(--fg2);display:inline-block;width:11px;margin-right:1px;vertical-align:top}
.pg{display:block;color:var(--fg);flex:none}
canvas.pb{display:block}
/* ---------- the readout ---------- */
.taskbar .readout{padding:0 6px}
.ro{display:grid;grid-auto-rows:16px;align-items:center;font:400 12px/16px "Segoe UI Variable Small","Segoe UI",sans-serif;
    font-variant-numeric:tabular-nums;color:var(--fg);white-space:nowrap}
.ro>i{display:block}
.ro .lb{font-size:11px;color:var(--fg2)}
.ro .gc{display:flex;align-items:center;height:16px}
.ro .rc{display:flex;align-items:center;height:16px}
.ro .v{display:inline-block;width:24px;text-align:right;margin-right:6px;flex:none}
.ro .v.b,.ro .b{font-weight:600}
.ro .v.dim{color:var(--stale)}
.ro .t{display:flex;align-items:center}
.ro .say{display:flex;align-items:center;gap:2px;font-weight:600}
.ro .st{color:var(--stale)}
.ro .st.act{color:var(--acc-text)}
.ro .rc>.sev{margin-right:2px}
.ro .rc>.v:only-child{text-align:left;width:auto}
.readout.hov{background:var(--hover)}
.anch{position:relative}

"""

# ---------------------------------------------------------------- JS reference drawing code
DRAW_JS = r"""
// ===== Reference drawing code. The native readout, tray icon and flyout must produce the same pixels. =====
// All geometry is computed in DEVICE pixels from the logical size and the scale s = DPI / 96.
const DPR = window.devicePixelRatio || 1;
const TOL = 0.03;  // pace tolerance (share), see readout-spec "Pace"
function css(el, name) { return getComputedStyle(el).getPropertyValue(name).trim(); }
function oddNear(target, hair) { return 2 * Math.round((target - hair) / 2) + hair; }   // keeps strokes centred on the track
function minus(parts, cuts) {
  for (const [c0, c1] of cuts) {
    parts = parts.flatMap(([p, q]) => { const o = []; if (c0 > p) o.push([p, Math.min(q, c0)]); if (c1 < q) o.push([Math.max(p, c1), q]); return o.filter(([x, y]) => y > x); });
  }
  return parts;
}
function pill(ctx, x, y, w, h, roundL, roundR) {
  const r = Math.min(h / 2, w / 2);
  ctx.beginPath();
  ctx.moveTo(x + (roundL ? r : 0), y);
  ctx.lineTo(x + w - (roundR ? r : 0), y);
  if (roundR) ctx.arc(x + w - r, y + h / 2, r, -Math.PI / 2, Math.PI / 2); else { ctx.lineTo(x + w, y); ctx.lineTo(x + w, y + h); }
  ctx.lineTo(x + (roundL ? r : 0), y + h);
  if (roundL) ctx.arc(x + r, y + h / 2, r, Math.PI / 2, Math.PI * 1.5); else { ctx.lineTo(x, y + h); ctx.lineTo(x, y); }
  ctx.closePath(); ctx.fill();
}
// pace bar geometry: returns device-pixel rectangles. u = used share, e = elapsed share (null = no tick)
function paceGeometry(Lpx, s, u, e, state) {
  const hair = Math.max(1, Math.floor(s));
  const fillH = oddNear(3 * s, hair), tickH = oddNear(9 * s, hair);
  let f = Math.round(Math.min(1, Math.max(0, u)) * Lpx); if (u > 0 && f === 0) f = 1;
  if (state === 'exh') f = Lpx;
  if (state === 'none') f = 0;
  let k = (e === null || state === 'none') ? null : Math.min(Lpx - hair, Math.max(0, Math.round(e * Lpx)));
  if (k !== null && state !== 'exh' && state !== 'none') {
    if (u < e - TOL && f > k - hair - 1) f = Math.max(u > 0 ? 1 : 0, k - hair - 1);          // under pace: stop before the gap
    if (u > e + TOL && f < k + 2 * hair + 1) f = Math.min(Lpx, k + 2 * hair + 1);            // ahead: show fill past the gap
  }
  const cuts = k === null ? [] : [[k - hair, k], [k + hair, k + 2 * hair]];
  return { hair, fillH, tickH, f, k, cuts };
}
function drawPace(cv) {
  const W = +cv.dataset.w, H = +(cv.dataset.h || 16);
  const Lpx = Math.round(W * DPR), Hpx = Math.round(H * DPR);
  cv.width = Lpx; cv.height = Hpx; cv.style.width = (Lpx / DPR) + 'px'; cv.style.height = (Hpx / DPR) + 'px';
  const ctx = cv.getContext('2d');
  const st = cv.dataset.s || 'ok';
  const u = +(cv.dataset.u || 0), e = cv.dataset.e === undefined ? null : +cv.dataset.e;
  const g = paceGeometry(Lpx, DPR, u, e, st);
  const yT = Math.floor((Hpx - g.hair) / 2), yF = yT - (g.fillH - g.hair) / 2, yK = yT - (g.tickH - g.hair) / 2;
  // track (right of the fill, minus the gaps)
  ctx.fillStyle = css(cv, st === 'stale' ? '--track' : '--track');
  for (const [a, b] of minus([[g.f, Lpx]], g.cuts)) ctx.fillRect(a, yT, b - a, g.hair);
  // fill
  const fillVar = { ok: '--fill', warn: '--caution', crit: '--critical', exh: '--critical', stale: '--stale', low: '--critical', acc: '--acc' }[st] || '--fill';
  ctx.fillStyle = css(cv, fillVar);
  for (const [a, b] of minus([[0, g.f]], g.cuts)) {
    if (st === 'stale') { for (let x = a; x < b; x += 3 * g.hair) ctx.fillRect(x, yF, Math.min(2 * g.hair, b - x), g.fillH); }
    else pill(ctx, a, yF, b - a, g.fillH, a === 0, b === g.f);
  }
  // tick
  if (g.k !== null) { ctx.fillStyle = css(cv, st === 'stale' ? '--stale' : '--tick'); ctx.fillRect(g.k, yK, g.hair, g.tickH); }
}

// ----- tray icon: one hand-tuned layout per size; other sizes use the next smaller layout, centred -----
const ICON = {
  16: { font: 11, ls: -0.4, base: 9,  tFont: 11, tBase: 12, bar: { x: 1, L: 14, yT: 13, fillH: 3, tickH: 5, hair: 1 }, r: 3, sw: 1 },
  20: { font: 13, ls: -0.4, base: 11, tFont: 13, tBase: 15, bar: { x: 1, L: 18, yT: 16, fillH: 3, tickH: 7, hair: 1 }, r: 4, sw: 1 },
  24: { font: 16, ls: -0.5, base: 13, tFont: 16, tBase: 18, bar: { x: 1, L: 22, yT: 19, fillH: 3, tickH: 7, hair: 1 }, r: 4, sw: 1 },
  32: { font: 21, ls: -0.6, base: 17, tFont: 21, tBase: 24, bar: { x: 2, L: 28, yT: 25, fillH: 4, tickH: 10, hair: 2 }, r: 6, sw: 2 },
};
const DROP = 'M8 1.2C8 1.2 3.2 6.9 3.2 10.1A4.8 4.8 0 0 0 12.8 10.1C12.8 6.9 8 1.2 8 1.2Z';
function drawIcon(cv) {
  let size = +(cv.dataset.size || 0);
  const native = !size;
  if (native) size = Math.round(16 * DPR);
  const key = [32, 24, 20, 16].find(k => k <= size) || 16;
  const d = ICON[key], off = Math.floor((size - key) / 2);
  cv.width = size; cv.height = size;
  const zoom = +(cv.dataset.zoom || 1);
  cv.style.width = (native ? 16 : size * zoom / (zoom > 1 ? 1 : DPR)) + 'px';
  cv.style.height = cv.style.width;
  const ctx = cv.getContext('2d');
  ctx.translate(off, off);
  const st = cv.dataset.s || 'ok', n = cv.dataset.n || '';
  const u = +(cv.dataset.u || 0), e = cv.dataset.e === undefined ? null : +cv.dataset.e;
  const fg = css(cv, '--fg'), fg2 = css(cv, '--stale');
  const digits = (txt, font, base, color) => {
    ctx.font = `600 ${font}px "Segoe UI Variable Small"`; ctx.textAlign = 'center'; ctx.textBaseline = 'alphabetic';
    ctx.letterSpacing = d.ls + 'px'; ctx.fillStyle = color; ctx.fillText(txt, key / 2 + 0.25, base);
  };
  const tile = (fill) => {
    ctx.beginPath(); ctx.roundRect(fill ? 0 : d.sw / 2, fill ? 0 : d.sw / 2, fill ? key : key - d.sw, fill ? key : key - d.sw, d.r);
    if (fill) { ctx.fillStyle = css(cv, '--critical'); ctx.fill(); }
    else { ctx.lineWidth = d.sw; ctx.strokeStyle = css(cv, '--caution'); ctx.stroke(); }
  };
  const drop = (color, hollow, scale) => {
    ctx.save(); const sc = key / 16 * scale; ctx.translate(key / 2 - 8 * sc, key / 2 - 8 * sc + (hollow ? 0 : 0)); ctx.scale(sc, sc);
    const p = new Path2D(DROP);
    if (hollow) { ctx.lineWidth = (key >= 32 ? 2 : key >= 24 ? 1.5 : 1.25) / sc; ctx.strokeStyle = color; ctx.stroke(p); }
    else { ctx.fillStyle = color; ctx.fill(p); }
    ctx.restore();
  };
  if (st === 'warn') { tile(false); digits(n, d.tFont, d.tBase, fg); return; }
  if (st === 'crit') { tile(true); digits(n, d.tFont, d.tBase, css(cv, '--on')); return; }
  if (st === 'exh') { tile(true); drop(css(cv, '--on'), true, 0.78); return; }
  if (st === 'start') { drop(fg, false, 0.95); return; }
  // ok / stale: number + pace bar
  digits(n, d.font, d.base, st === 'stale' ? fg2 : fg);
  const b = d.bar, hair = b.hair;
  let f = Math.round(Math.min(1, u) * b.L); if (u > 0 && f === 0) f = 1;
  let k = e === null ? null : Math.min(b.L - hair, Math.max(0, Math.round(e * b.L)));
  if (k !== null) {
    if (u < e - TOL && f > k - hair - 1) f = Math.max(1, k - hair - 1);
    if (u > e + TOL && f < k + 2 * hair + 1) f = Math.min(b.L, k + 2 * hair + 1);
  }
  const cuts = k === null ? [] : [[k - hair, k], [k + hair, k + 2 * hair]];
  const yF = b.yT - (b.fillH - hair) / 2, yK = b.yT - (b.tickH - hair) / 2;
  ctx.fillStyle = css(cv, '--track');
  for (const [a, c] of minus([[f, b.L]], cuts)) ctx.fillRect(b.x + a, b.yT, c - a, hair);
  ctx.fillStyle = st === 'stale' ? fg2 : fg;
  for (const [a, c] of minus([[0, f]], cuts)) {
    if (st === 'stale') { for (let x = a; x < c; x += 3 * hair) ctx.fillRect(b.x + x, yF, Math.min(2 * hair, c - x), b.fillH); }
    else ctx.fillRect(b.x + a, yF, c - a, b.fillH);
  }
  if (k !== null) { ctx.fillStyle = st === 'stale' ? fg2 : fg; ctx.fillRect(b.x + k, yK, hair, b.tickH); }
}

// ----- sparkline: daily consumption, last 14 days, as columns (flyout card header) -----
function drawSpark(cv) {
  const W = +cv.dataset.w, H = +cv.dataset.h;
  const vals = cv.dataset.v.split(',').map(x => x === '' ? null : +x);
  const floor = +(cv.dataset.floor || 0);
  const Wp = Math.round(W * DPR), Hp = Math.round(H * DPR);
  cv.width = Wp; cv.height = Hp; cv.style.width = (Wp / DPR) + 'px'; cv.style.height = (Hp / DPR) + 'px';
  const ctx = cv.getContext('2d');
  const n = vals.length, hair = Math.max(1, Math.floor(DPR));
  const col = Math.round(3 * DPR), gap = Math.max(1, Math.round(1 * DPR));
  const total = n * col + (n - 1) * gap, x0 = Wp - total;
  const max = Math.max(floor, ...vals.filter(v => v !== null));
  // baseline under all slots
  ctx.fillStyle = css(cv, '--dis'); ctx.fillRect(x0, Hp - hair, total, hair);
  const stale = cv.dataset.s === 'stale';
  vals.forEach((v, i) => {
    if (v === null || v <= 0) return;
    const h = Math.max(hair, Math.round(v / max * (Hp - hair)));
    ctx.fillStyle = css(cv, stale ? '--fg3' : (i === n - 1 ? '--spark-now' : '--spark'));
    ctx.fillRect(x0 + i * (col + gap), Hp - h, col, h);
  });
}
document.querySelectorAll('canvas.pb').forEach(drawPace);
document.querySelectorAll('canvas.ti').forEach(drawIcon);
document.querySelectorAll('canvas.sp').forEach(drawSpark);
"""


def bar(u, e, state="ok", w=40, h=16, cls="pb"):
    ea = "" if e is None else f' data-e="{e:.4f}"'
    return f'<canvas class="{cls}" data-w="{w}" data-h="{h}" data-u="{u:.4f}"{ea} data-s="{state}" aria-hidden="true"></canvas>'


def icon(state, n="", u=0.0, e=None, size=0, zoom=1):
    ea = "" if e is None else f' data-e="{e:.4f}"'
    sa = f' data-size="{size}" data-zoom="{zoom}"' if size else ""
    return f'<canvas class="ti" data-s="{state}" data-n="{n}" data-u="{u:.4f}"{ea}{sa} aria-hidden="true"></canvas>'
