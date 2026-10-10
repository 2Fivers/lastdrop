"""Generates docs/design/final/flyout.html: the final Lastdrop flyout (Slint window), light and dark."""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import TOKENS_CSS, DRAW_JS, glyph, sev, RESET, bar, icon

OUT = sys.argv[1]
BW = 310  # meter bar width = card inner width


def spark(vals, floor, st=""):
    v = ",".join("" if x is None else str(x) for x in vals)
    return f'<canvas class="sp" data-w="55" data-h="16" data-v="{v}" data-floor="{floor}"{(" data-s=" + chr(34) + st + chr(34)) if st else ""} aria-hidden="true"></canvas>'


def meter(name, value, reset, u, e, st="acc", right=None, line=None, uia="", vdim=False):
    """One window meter. right: replaces the reset text in trouble (severity glyph + words)."""
    rc = right if right is not None else f'<span class="rs">{RESET}{reset}</span>'
    ln = f'<div class="ml">{line}</div>' if line else ""
    return (f'<div class="m" role="progressbar" aria-label="{uia}" data-rl="meter">'
            f'<div class="mh"><span class="mn">{name}</span>{rc}<span class="mv{" dim" if vdim else ""}">{value}</span></div>'
            f'{bar(u, e, st, w=BW, h=9)}{ln}</div>')


def card(g, name, plan, age, body, sp, msg="", age_cls="", uia=""):
    m = f'<div class="msg">{msg}</div>' if msg else ""
    return (f'<section class="card" aria-label="{uia}" data-rl="card"><div class="chd" data-rl="card header">{glyph(g, 16)}<span class="cn">{name}</span>'
            f'<span class="plan">{plan}</span><span class="sp1"></span><span class="age {age_cls}">{age}</span>{sp}</div>{m}'
            f'<div class="cb">{body}</div></section>')


def balance(amount, unit, fc, rate, until, fc_sev=""):
    s = sev(fc_sev) if fc_sev else ""
    return (f'<div class="bal" data-rl="balance"><div class="b1"><span class="ba">{amount}</span><span class="bu">{unit}</span>'
            f'<span class="bf{" b" if fc_sev else ""}">{s}{fc}</span></div>'
            f'<div class="b2"><span>{rate}</span><span>{until}</span></div></div>')


SP_CLAUDE = [6, 9, 4, 0, 0, 11, 8, 7, 12, 5, 0, 3, 9, 6]
SP_CODEX = [1, 0, 2, 0, 0, 1, 3, 0, 1, 0, 0, 2, 1, 1]
SP_MESHY = [120, 0, 250, 90, 140, 0, 0, 180, 160, 95, 210, 130, 60, 140]
SP_COPILOT = [2, 3, 1, 0, 0, 4, 3, 2, 3, 2, 0, 1, 3, 2]


def claude_card(state="ok"):
    if state == "warn":
        m5 = meter("5-hour session", "78%", None, .78, .45, "warn",
                   right=f'<span class="say">{sev("warn")}Out 15:20</span>',
                   line="Runs out 1&nbsp;h&nbsp;10&nbsp;min before the reset at 16:30.",
                   uia="Claude 5-hour session: warning, runs out at about 3:20 PM, before the reset at 4:30 PM; 78 percent used")
        age = "1 min ago"
    elif state == "stale":
        m5 = meter("5-hour session", "42%", "Resets 14:56", .42, .553, "stale", vdim=True, uia="5-hour session: 42 percent used at 12:42; resets at 2:56 PM")
        age = "47 min ago"
    else:
        m5 = meter("5-hour session", "42%", "Resets 14:56", .42, .553, uia="5-hour session: on track, 42 percent used, 55 percent of the time gone, resets at 2:56 PM")
        age = "1 min ago"
    st = "stale" if state == "stale" else "acc"
    mw = meter("Weekly &middot; all models", "61%", "Resets Mon 09:00", .61, .736 if state != "warn" else .741, st, vdim=state == "stale",
               uia="Weekly, all models: on track, 61 percent used, 74 percent of the time gone, resets Monday 9:00 AM")
    mf = meter("Weekly &middot; Fable", "18%", "Resets Mon 09:00", .18, .736 if state != "warn" else .741, st, vdim=state == "stale",
               uia="Weekly, Fable: on track, 18 percent used, resets Monday 9:00 AM")
    msg = ""
    age_cls = ""
    if state == "stale":
        msg = (f'{sev("stale")}<span><b>Claude isn&rsquo;t answering</b> &middot; showing 12:42 &middot; next try 13:30</span>')
        age_cls = "warnage"
        age = f'{age}'
    return card("claude", "Claude", "Max", age, m5 + mw + mf, spark(SP_CLAUDE, 10, "stale" if state == "stale" else ""), msg, age_cls,
                uia="Claude, Max plan, updated " + ("47 minutes ago, not answering" if state == "stale" else "1 minute ago"))


def codex_card():
    return card("codex", "Codex", "Pro Lite", "2 min ago",
                meter("Weekly", "5%", "Resets Fri 14:26", .05, .133, uia="Codex weekly: on track, 5 percent used, resets Friday 2:26 PM"),
                spark(SP_CODEX, 10), uia="Codex, Pro Lite plan, updated 2 minutes ago")


def meshy_card():
    return card("meshy", "Meshy", "Pro", "1 min ago",
                balance("1,240", "credits", "&asymp;&thinsp;9 days left", "138 credits a day, last 7 days", "until &asymp;&thinsp;Mon 19 Oct"),
                spark(SP_MESHY, 163), uia="Meshy, Pro plan, updated 1 minute ago")


def copilot_card():
    return card("copilot", "Copilot", "Pro+", "1 min ago",
                meter("Premium requests", "31%", "Resets 1 Nov", .31, .307, uia="Copilot monthly premium requests: 31 percent used, resets November 1"),
                spark(SP_COPILOT, 10), uia="Copilot, Pro+ plan")


def flyout(theme, state="ok", cards=None, verdict=None, extra_cls="", height=None, scroll=False):
    if cards is None:
        cards = [claude_card(state), codex_card(), meshy_card()]
    if verdict is None:
        verdict = {"ok": "All limits last until their reset",
                   "warn": f'{sev("warn")}Claude 5-hour runs out at 15:20',
                   "stale": f'{sev("stale")}Claude: no reading for 47 min'}[state]
    hs = f' style="height:{height}px"' if height else ""
    sb = '<div class="sbar"></div>' if scroll else ""
    return (f'<div class="fly {theme} {extra_cls}" role="dialog" aria-label="Lastdrop"{hs}>'
            f'<header class="hd" data-rl="header"><span class="ttl">Lastdrop</span><span class="vd">{verdict}</span>'
            f'<button class="ib" aria-label="Refresh" data-rl="icon button">&#xE72C;</button></header>'
            f'<div class="list" data-rl="list">{"".join(cards)}{sb}</div>'
            f'<footer class="ft" data-rl="footer"><button class="sbtn" data-rl="button"><i>&#xE713;</i>Settings</button></footer></div>')


def lab(t, sub=""):
    return f'<div class="flab"><b>{t}</b>{(" &middot; " + sub) if sub else ""}<span class="hgt"></span></div>'


APPS = '<div class="apps"><div class="app"><i></i></div><div class="app"><i></i></div><div class="app"><i></i></div><div class="app"><i></i></div><div class="app"><i></i></div></div>'


def ro_mini():
    # readout (same as readout.html 1a) for the in-context scene
    cols = "17px 6px 40px 6px 79px"
    def row(lb, u, e, v, t):
        return (f'<span class="lb">{lb}</span><i></i>{bar(u, e, "ok")}<i></i>'
                f'<span class="rc"><span class="v">{v}</span><span class="t">{RESET}{t}</span></span>')
    return (f'<div class="readout"><div class="ro" style="grid-template-columns:{cols}">{row("5h", .42, .553, "42%", "14:56")}'
            f'{row("Wk", .61, .736, "61%", "Mon")}</div></div>')


scene = (f'<div class="scene"><div class="desk"></div>'
         f'<div class="fly-pos">{flyout("L")}</div>'
         f'<div class="taskbar light L" style="position:absolute;left:0;right:0;bottom:0;width:auto">{APPS}'
         f'<div class="tray">{ro_mini()}<span class="glyph small">&#xE70E;</span><span class="ld-icon">{icon("ok", "61", .61, .736)}</span>'
         f'<span class="glyph">&#xE701;</span><span class="glyph">&#xE995;</span><div class="clock">12:42<br>2026-10-10</div>'
         f'<span class="glyph small">&#xEA8F;</span></div></div></div>')

rows = []
rows.append(('<h2>1 &middot; Normal &mdash; three services</h2>'
             '<p class="intro">Claude (5-hour, weekly, weekly Fable), Codex (weekly) and Meshy (credit balance), all on track. '
             'Above: the owner&rsquo;s setup in place &mdash; light, transparency off, so the acrylic backdrop renders as its solid fallback &mdash; opened from the tray icon, '
             '12&nbsp;px above the taskbar and from the screen&rsquo;s right edge, like Quick Settings. The whole flyout is <b>360 &times; 478</b> at 100&nbsp;%: no scrolling for three services.</p>'))
rows.append(scene)
rows.append('<div class="frow">' + "".join([
    f'<div class="fcell">{lab("Dark", "transparency off")}{flyout("D")}</div>',
    f'<div class="fcell wallp">{lab("Light", "transparency on (acrylic)")}{flyout("L", extra_cls="acr")}</div>',
    f'<div class="fcell wallp">{lab("Dark", "transparency on (acrylic)")}{flyout("D", extra_cls="acr")}</div>']) + '</div>')

rows.append('<h2>2 &middot; Warning</h2><p class="intro">13:45. Claude&rsquo;s 5-hour session runs out before its reset. The header verdict names it; '
            'the meter takes the caution fill and the same words as the taskbar (&ldquo;Out 15:20&rdquo;), and one secondary line says by how much. '
            'The flyout grows by that one line (to 498&nbsp;px) instead of scrolling. Everything else stays calm.</p>')
rows.append('<div class="frow">' + "".join([
    f'<div class="fcell">{lab("Light")}{flyout("L", "warn")}</div>',
    f'<div class="fcell">{lab("Dark")}{flyout("D", "warn")}</div>',
    f'<div class="fcell notes"><div class="nt"><b>Trouble rule, as in the taskbar</b><br>'
    f'Colour appears only with a severity glyph and words: caution fill + triangle + &ldquo;Out 15:20&rdquo;; critical fill + disc + &ldquo;12 min left&rdquo; '
    f'when the run-out is under an hour away; &ldquo;Back 16:30&rdquo; when empty. Text stays in the text colours (InfoBar convention), so it keeps its contrast.<br><br>'
    f'<b>Calm rule</b><br>On track, bars use the accent (WinUI ProgressBar), which reads as neutral in an app surface; the taskbar uses its monochrome text colour. '
    f'Both carry the same 1&nbsp;px tick at the share of the window gone.<br><br>'
    f'<b>Sparkline</b><br>Top right of every card: use per day, last 14 days (today in accent, on the right). For windows it is the share of the longest window used that day; '
    f'for balances, credits spent. Each scales to its own 14-day maximum, with a floor so a quiet fortnight stays low.</div></div>']) + '</div>')

rows.append('<h2>3 &middot; Stale data</h2><p class="intro">13:29. Claude&rsquo;s usage endpoint has not answered for 47 minutes (HTTP 503, backing off); Codex and Meshy read fine. '
            'Claude&rsquo;s card keeps its last numbers, dimmed, with dashed bars frozen at 12:42, and says why in one line under its header; the reset times still hold, '
            'because they do not depend on the reading. The header verdict names the problem. When the whole machine is offline every card looks like Claude&rsquo;s, with '
            '&ldquo;Offline&rdquo; as the reason.</p>')
rows.append('<div class="frow">' + "".join([
    f'<div class="fcell">{lab("Light")}{flyout("L", "stale")}</div>',
    f'<div class="fcell">{lab("Dark")}{flyout("D", "stale")}</div>',
    f'<div class="fcell">{lab("Four services", "list scrolls")}'
    + flyout("L", cards=[claude_card(), codex_card(), copilot_card(), meshy_card()], height=520, scroll=True,
             verdict="All limits last until their reset") + '</div>']) + '</div>')

rows.append('<h2>4 &middot; Redline</h2><p class="intro">Boxes measured from this page at 100&nbsp;% (logical px). Every value is also in flyout-spec.md.</p>')
rows.append(f'<div class="frow"><div class="fcell rl-cell">{flyout("L", extra_cls="redline")}</div><div class="fcell rl-notes"><div class="rlt"></div></div></div>')

PAGE_CSS = r"""
body{background:#5f636b;padding:24px 24px 40px;margin:0}
h1{font:600 22px/28px var(--font);color:#fff;margin:0 0 6px}
h2{font:600 16px/22px var(--font);color:#fff;margin:30px 0 4px}
.intro{font:13px/19px var(--font);color:#eef0f3;max-width:1110px;margin:2px 0 12px}
.intro b,.lead b{color:#fff}
.lead{font:14px/21px var(--font);color:#f4f4f4;max-width:1110px;margin:0 0 12px}
.frow{display:flex;gap:24px;align-items:flex-start;margin-bottom:8px}
.fcell{width:360px;flex:none}
.flab{font:12px/16px var(--font);color:#e6e8ec;margin:0 0 6px;height:16px}
.flab b{color:#fff;font-weight:600}
.flab .hgt{color:#c9ccd2;margin-left:6px}
.fcell.wallp{padding:0}
.notes .nt{font:12px/18px var(--font);color:#eef0f3;padding-top:22px}
.notes .nt b{color:#fff}

/* ---------- scene ---------- */
.scene{position:relative;width:1152px;height:560px;border-radius:8px;overflow:hidden;background:#6b6f78}
.desk{position:absolute;inset:0;background:linear-gradient(135deg,#2d3b67 0%,#5b4c8a 45%,#a46aa0 75%,#d58a8a 100%)}
.fly-pos{position:absolute;right:12px;bottom:61px}
.scene .taskbar .readout{padding:0 6px}

/* ---------- flyout window ---------- */
.fly{--t1:rgba(0,0,0,.894);--t2:rgba(0,0,0,.62);--t3:rgba(0,0,0,.447);
     --bg:#F9F9F9;--cardbg:rgba(255,255,255,.7);--cardst:rgba(0,0,0,.0578);--badge:rgba(0,0,0,.0578);
     --sub-hover:rgba(0,0,0,.0373);--footer:rgba(0,0,0,.04);--wstroke:rgba(0,0,0,.10);
     --fg:rgba(0,0,0,.894);--fg2:rgba(0,0,0,.62);--fg3:rgba(0,0,0,.447);--fill:var(--acc);--tick:var(--t1);
     width:360px;box-sizing:border-box;border-radius:8px;background:var(--bg);color:var(--t1);
     box-shadow:0 0 0 1px var(--wstroke),0 32px 64px rgba(0,0,0,.18),0 2px 21px rgba(0,0,0,.15);
     display:flex;flex-direction:column;overflow:hidden;position:relative;
     font:400 14px/20px "Segoe UI Variable Text","Segoe UI",sans-serif;font-variant-numeric:tabular-nums}
.fly.D{--t1:#ffffff;--t2:rgba(255,255,255,.786);--t3:rgba(255,255,255,.545);
       --bg:#2C2C2C;--cardbg:rgba(255,255,255,.0512);--cardst:rgba(0,0,0,.10);--badge:rgba(255,255,255,.0605);
       --sub-hover:rgba(255,255,255,.0605);--footer:rgba(0,0,0,.19);--wstroke:rgba(255,255,255,.08);
       --fg:#fff;--fg2:rgba(255,255,255,.786);--fg3:rgba(255,255,255,.545);
       box-shadow:0 0 0 1px rgba(0,0,0,.6),inset 0 0 0 1px var(--wstroke),0 32px 64px rgba(0,0,0,.37),0 2px 21px rgba(0,0,0,.37)}
.fly.acr{background:rgba(243,243,243,.62);backdrop-filter:blur(60px) saturate(1.25)}
.fly.D.acr{background:rgba(32,32,32,.70)}
.wallp{background:linear-gradient(140deg,#2d3b67 0%,#5b4c8a 40%,#b0628f 72%,#e6a06a 100%);border-radius:10px;padding:0 !important}
.wallp .flab{padding:8px 12px 0;margin:0 0 6px}
.wallp .fly{margin:0 0 0 0}

.hd{display:flex;align-items:center;height:40px;padding:0 4px 0 16px;flex:none}
.ttl{font:600 14px/20px "Segoe UI Variable Text";color:var(--t1)}
.vd{display:flex;align-items:center;gap:4px;margin-left:8px;font:400 12px/16px "Segoe UI Variable Small";color:var(--t2);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;flex:1;min-width:0}
.vd .sev{margin-right:0}
button{font:inherit;color:inherit;border:0;margin:0;padding:0;background:transparent}
.ib{width:32px;height:32px;border-radius:4px;font:normal 16px/32px "Segoe Fluent Icons";color:var(--t1);text-align:center;flex:none}
.list{flex:1;display:flex;flex-direction:column;gap:8px;padding:0 12px 12px;overflow:hidden;position:relative}
.card{border-radius:8px;background:var(--cardbg);border:1px solid var(--cardst);padding:12px 12px 12px;flex:none}
.chd{display:flex;align-items:center;height:20px;gap:0}
.chd .pg{width:16px;height:16px;color:var(--t1);margin-right:8px}
.cn{font:600 14px/20px "Segoe UI Variable Text"}
.plan{margin-left:8px;font:400 12px/16px "Segoe UI Variable Small";color:var(--t2);background:var(--badge);border-radius:4px;padding:1px 6px 1px}
.sp1{flex:1}
.age{font:400 12px/16px "Segoe UI Variable Small";color:var(--t2);margin-right:8px;white-space:nowrap}
.age.warnage{color:var(--t2)}
canvas.sp{display:block}
.msg{display:flex;gap:6px;align-items:flex-start;margin-top:8px;font:400 12px/16px "Segoe UI Variable Small";color:var(--t2)}
.msg b{color:var(--t1);font-weight:600}
.cb{margin-top:10px;display:flex;flex-direction:column;gap:8px}
.mh{display:flex;align-items:center;height:20px}
.mn{flex:1;min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;color:var(--t1)}
.rs{display:flex;align-items:center;font:400 12px/16px "Segoe UI Variable Small";color:var(--t2);white-space:nowrap}
.rs .rg{font-size:12px;width:12px;margin-right:4px;color:var(--t2)}
.say{display:flex;align-items:center;gap:4px;font:600 12px/16px "Segoe UI Variable Small";color:var(--t1);white-space:nowrap}
.mv{font:600 14px/20px "Segoe UI Variable Text";min-width:36px;margin-left:12px;text-align:right;color:var(--t1)}
.mv.dim{color:var(--t2)}
.m canvas.pb{margin-top:4px}
.ml{margin-top:4px;font:400 12px/16px "Segoe UI Variable Small";color:var(--t2)}
.bal .b1{display:flex;align-items:baseline;height:28px}
.ba{font:600 20px/28px "Segoe UI Variable Display";color:var(--t1)}
.bu{margin-left:6px;color:var(--t2)}
.bf{margin-left:auto;display:flex;align-items:center;gap:4px;color:var(--t1)}
.b2{display:flex;justify-content:space-between;font:400 12px/16px "Segoe UI Variable Small";color:var(--t2);margin-top:2px}
.ft{height:48px;flex:none;display:flex;align-items:center;justify-content:flex-end;padding:0 8px;border-top:1px solid var(--div);background:var(--footer)}
.sbtn{display:flex;align-items:center;gap:8px;height:32px;padding:0 11px;border-radius:4px;font:400 14px/20px "Segoe UI Variable Text";color:var(--t1)}
.sbtn i{font:normal 16px/16px "Segoe Fluent Icons"}
.sbar{position:absolute;right:3px;top:2px;width:2px;height:150px;border-radius:1px;background:var(--t3)}

/* redline overlay */
.rl-box{position:absolute;pointer-events:none;outline:1px solid rgba(255,0,170,.85);background:rgba(255,0,170,.06)}
.rl-tag{position:absolute;pointer-events:none;font:600 10px/12px "Segoe UI";color:#fff;background:#d4007a;padding:1px 4px;border-radius:3px;white-space:nowrap}
.rl-cell{position:relative;width:470px}
.rlt{font:12px/18px var(--font);color:#eef0f3;padding-top:2px}
.rlt table{border-collapse:collapse}
.rlt td,.rlt th{padding:3px 14px 3px 0;border-bottom:1px solid rgba(255,255,255,.16);text-align:left;vertical-align:top}
.rlt th{color:#fff;font-weight:600}
"""

REDLINE_JS = r"""
// height labels + redline boxes, measured from the live layout
document.querySelectorAll('.fcell').forEach(c => { const f = c.querySelector('.fly'); const h = c.querySelector('.hgt'); if (f && h) h.textContent = '360 × ' + Math.round(f.getBoundingClientRect().height) + ' px'; });
const rf = document.querySelector('.fly.redline');
if (rf) {
  const cell = rf.parentElement, cr = cell.getBoundingClientRect();
  const add = (el, label, side) => {
    const r = el.getBoundingClientRect();
    const b = document.createElement('div'); b.className = 'rl-box';
    Object.assign(b.style, { left: (r.left - cr.left) + 'px', top: (r.top - cr.top) + 'px', width: r.width + 'px', height: r.height + 'px' });
    cell.appendChild(b);
    const t = document.createElement('div'); t.className = 'rl-tag'; t.textContent = label + ' ' + Math.round(r.width) + '×' + Math.round(r.height);
    Object.assign(t.style, { left: (side === 'r' ? r.right - cr.left + 4 : r.left - cr.left) + 'px', top: (r.top - cr.top + (side === 'r' ? 0 : -13)) + 'px' });
    cell.appendChild(t);
  };
  add(rf.querySelector('.hd'), 'header', 'r');
  rf.querySelectorAll('.card').forEach((c, i) => add(c, 'card', 'r'));
  add(rf.querySelector('.chd'), 'card header', 'in');
  rf.querySelectorAll('.card')[0].querySelectorAll('.m').forEach((m, i) => { if (i === 0) add(m, 'meter', 'in'); });
  add(rf.querySelector('.bal'), 'balance', 'in');
  add(rf.querySelector('.ft'), 'footer', 'r');
  add(rf.querySelector('canvas.sp'), 'spark', 'in');
}
"""

SPEC_TABLE = """<table>
<tr><th>Element</th><th>Size / spacing (logical px at 100&nbsp;%)</th></tr>
<tr><td>Window</td><td>360 wide; height = content, 478 for this set; max 560, then the list scrolls. Corner 8 (DWM), 12 from the work-area edges.</td></tr>
<tr><td>Header</td><td>40 high; 16 left, 4 right; title 14/20 semibold; verdict 12/16 secondary, 8 after the title; refresh 32&times;32.</td></tr>
<tr><td>List</td><td>12 left/right, 0 top, 12 bottom; 8 between cards.</td></tr>
<tr><td>Card</td><td>336 wide; 1 stroke; radius 8; padding 12; inner width 310.</td></tr>
<tr><td>Card header</td><td>20 high: glyph 16, 8, name 14/20 semibold, 8, plan badge (12/16, padding 1&nbsp;6, radius 4), flex, age 12/16 secondary, 8, sparkline 55&times;16.</td></tr>
<tr><td>Card body</td><td>10 below the header (or below the message line, which sits 8 below the header); meters 8 apart.</td></tr>
<tr><td>Meter</td><td>name row 20 (name 14/20, reset 12/16 secondary with 12&nbsp;px glyph + 4, value 14/20 semibold, min 36, 12 before it); 4; bar 310&times;9; trouble line 4 + 16.</td></tr>
<tr><td>Balance</td><td>row 28 (amount 20/28 Display semibold, 6, unit 14/20 secondary; forecast right 14/20); 2; row 16 (12/16 secondary).</td></tr>
<tr><td>Sparkline</td><td>14 columns &times; 3, gap 1 = 55&times;16; 1&nbsp;px baseline; today on the right in accent.</td></tr>
<tr><td>Footer</td><td>48 high incl. 1 divider; Settings button 32 high, padding 0&nbsp;11, glyph 16 + 8 + label 14/20; 8 from the right.</td></tr>
</table>"""

html = f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Lastdrop flyout, final</title>
<link rel="stylesheet" href="harness.css">
<style>{TOKENS_CSS}{PAGE_CSS}</style>
</head>
<body>
<h1>Lastdrop &mdash; flyout, final design</h1>
<p class="lead">The flyout speaks the taskbar&rsquo;s language at a larger size: the same pace bar (1&nbsp;px track, 3&nbsp;px fill, 1&nbsp;px tick at the share of the window gone), the same reset glyph, the same words when something is wrong, and colour only for trouble.
It is a Windows 11 tray flyout: acrylic backdrop (solid when transparency is off), 8&nbsp;px corners, Segoe UI Variable and Segoe Fluent Icons, WinUI 3 colour tokens, following the <b>Windows mode</b> (the taskbar&rsquo;s light/dark) like Quick Settings beside it.
One card per service: name, plan, data age and a 14-day sparkline on one line, then its meters. Balance services (Meshy) show the amount and how long it lasts instead of a bar.</p>
{"".join(rows).replace('<div class="rlt"></div>', '<div class="rlt">' + SPEC_TABLE + '</div>')}
<script>{DRAW_JS}{REDLINE_JS}</script>
</body>
</html>
"""
open(OUT, "w", encoding="utf-8").write(html)
print("ok", len(html))
