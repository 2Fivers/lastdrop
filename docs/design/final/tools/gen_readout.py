"""Generates docs/design/final/readout.html: the final Lastdrop taskbar readout, hover card and tray icon."""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import TOKENS_CSS, DRAW_JS, glyph, sev, RESET, bar, icon

OUT = sys.argv[1]

# ---------------------------------------------------------------- readout pieces
RIGHT_W = 79      # value + time cell, two-row and single-meter forms (en: widest = stale "24 + 6 + 47m ago")
COMPACT_W = 53    # compact cell (en: widest = critical "(!) 12 min")


def right(m):
    k = m["k"]
    if k == "n":        # normal: value + reset
        return f'<span class="v">{m["v"]}</span><span class="t">{RESET}{m["t"]}</span>'
    if k == "low":      # critical by level (>= 90 %) but the forecast lasts: numbers stay, value semibold
        return f'<span class="v b">{m["v"]}</span><span class="t">{RESET}{m["t"]}</span>'
    if k == "say":      # trouble: severity glyph + the answer in words
        return f'<span class="say">{sev(m["sev"])}<span>{m["say"]}</span></span>'
    if k == "stale":
        cls = "st act" if m.get("act") else "st"
        return f'<span class="v dim">{m["v"]}</span><span class="{cls}">{m["st"]}</span>'
    if k == "bal":      # balance: share of the last top-up spent + how long the rest lasts
        return f'<span class="v">{m["v"]}</span><span class="t"><i class="rg ap" aria-hidden="true">&asymp;</i>9d</span>'
    if k == "none":
        return f'<span class="v dim">&ndash;</span><span class="st">{m.get("st", "")}</span>'
    raise ValueError(k)


def cright(m):
    k = m["k"]
    if k == "n":
        return f'<span class="v">{m["v"]}</span>'
    if k == "stale":
        return f'<span class="v dim">{m["v"]}</span>'
    return f'{sev(m["sev"])}<span class="b">{m["say"]}</span>'


def readout(rows, lw=17, multi=False, compact=False, name="", cls="", extra=""):
    cols = []
    if multi:
        cols += ["12px", "4px"]
    cols += [f"{lw}px", "6px", "40px", "6px", f"{COMPACT_W if compact else RIGHT_W}px"]
    cells = []
    for m in rows:
        if multi:
            cells.append(f'<span class="gc">{glyph(m["g"])}</span><i></i>')
        st = m.get("bs", "ok")
        cells.append(f'<span class="lb">{m["lb"]}</span><i></i>{bar(m["u"], m.get("e"), st)}<i></i>')
        cells.append(f'<span class="rc">{cright(m) if compact else right(m)}</span>')
    lab = f' role="button" aria-label="{name}"' if name else ""
    return (f'<div class="readout{(" " + cls) if cls else ""}"{lab}>{extra}<div class="ro" style="grid-template-columns:{" ".join(cols)}">'
            f'{"".join(cells)}</div></div>')


def width_of(lw, multi, compact):
    return 6 + (16 if multi else 0) + lw + 6 + 40 + 6 + (COMPACT_W if compact else RIGHT_W) + 6


# ---------------------------------------------------------------- taskbar frames
APPS = '<div class="apps"><div class="app"><i></i></div><div class="app"><i></i></div><div class="app"><i></i></div><div class="app"><i></i></div><div class="app"><i></i></div></div>'


def tray(ro, ic, clock, date="2026-10-10"):
    return (f'<div class="tray">{ro}<span class="glyph small">&#xE70E;</span><span class="ld-icon">{ic}</span>'
            f'<span class="glyph">&#xE701;</span><span class="glyph">&#xE995;</span>'
            f'<div class="clock">{clock}<br>{date}</div><span class="glyph small">&#xEA8F;</span></div>')


def taskbar(theme, ro, ic, clock, full=False, w=560, cls=""):
    tb = {"L": "light", "D": "dark", "HA": "dark hca", "HD": "light hcd", "AL": "light acr", "AD": "dark acr"}[theme]
    tok = {"AL": "L", "AD": "D"}.get(theme, theme)
    style = "" if full else f' style="width:{w}px"'
    return (f'<div class="taskbar {tb} {tok}{(" " + cls) if cls else ""}"{style}>{APPS if full else ""}'
            f'{tray(ro, ic, clock)}</div>')


def pair(label, note, mk, clock, ic_args, w=560, tag=None):
    """light + dark fragment side by side. mk(theme) -> readout html."""
    out = [f'<div class="sc"><div class="sch"><b>{label}</b>{(" <span class=wtag>" + tag + "</span>") if tag else ""}</div>'
           f'<p class="note">{note}</p><div class="prow">']
    for th in ("L", "D"):
        out.append(f'<div class="fr"><span class="ttag">{"light" if th == "L" else "dark"}</span>{taskbar(th, mk(th), icon(*ic_args), clock, w=w)}</div>')
    out.append('</div></div>')
    return "".join(out)


# ---------------------------------------------------------------- scenario data (Saturday 2026-10-10)
# Claude 5h: resets 14:56 (window 09:56-14:56). Weekly: resets Mon 12 Oct 09:00. Codex weekly: resets Fri 16 Oct 14:26.
C5 = dict(g="claude", lb="5h", u=.42, e=.553, k="n", v="42%", t="14:56")
CW = dict(g="claude", lb="Wk", u=.61, e=.736, k="n", v="61%", t="Mon")
CF = dict(g="claude", lb="Fable", u=.18, e=.736, k="n", v="18%", t="Mon")
XW = dict(g="codex", lb="Wk", u=.05, e=.133, k="n", v="5%", t="Fri")
PM = dict(g="copilot", lb="Mo", u=.31, e=.307, k="n", v="31%", t="22d")

N_OK = "Claude 5-hour session: on track, 42 percent used, resets at 14:56. Claude weekly: on track, 61 percent used, resets Monday 09:00."

S = []   # sections html


def H(title, intro=""):
    S.append(f'<h2>{title}</h2>' + (f'<p class="intro">{intro}</p>' if intro else ""))


# ================================================================ 0. hero: full taskbars
hero = []
for th in ("L", "D"):
    hero.append(f'<div class="fr full"><span class="ttag">{"light taskbar" if th == "L" else "dark taskbar"} &middot; 100 %</span>'
                f'{taskbar(th, readout([C5, CW], name=N_OK), icon("ok", "61", .61, .736), "12:42", full=True)}</div>')

# ================================================================ 1. single service, every state
H("1 &middot; One service, two meters &mdash; every state",
  "Claude, 5-hour session + weekly. Width is fixed at <b>160&nbsp;px</b> in every state below, so the tray never moves. "
  "Fill = share used, tick = share of the window gone. Words replace the numbers only when something is wrong.")
S.append(pair("1a &middot; On track", "12:42. 5h: 42&nbsp;% used, 55&nbsp;% of the window gone. Weekly: 61&nbsp;% used, 74&nbsp;% gone, resets Monday. "
              "Both fills stop short of their ticks. Nothing is coloured. Tray icon: the weekly (forecast 83&nbsp;% at reset beats 78&nbsp;%).",
              lambda th: readout([C5, CW], name=N_OK), "12:42", ("ok", "61", .61, .736)))
S.append(pair("1b &middot; Ahead of the tick, but it lasts", "13:05. 5h: 68&nbsp;% used at 63&nbsp;% gone after a burst, idle since: the fill runs past the tick, "
              "but the recent rate says it lasts to the reset, so the row stays calm. The tick shows the trend; colour only shows a forecast that fails.",
              lambda th: readout([dict(C5, u=.68, e=.63, v="68%"), CW], name="Claude 5-hour session: on track, ahead of even pace, 68 percent used, resets at 14:56. Claude weekly: on track, 61 percent used."),
              "13:05", ("ok", "68", .68, .63)))
S.append(pair("1c &middot; Warning &mdash; runs out before the reset", "13:45. 5h: 78&nbsp;% used at 45&nbsp;% gone; at the recent rate it is empty at 15:20, the reset is 16:30. "
              "Caution fill, caution triangle, and the row says when: <b>Out 15:20</b> (a clock time: more than an hour away). Tray: outlined tile.",
              lambda th: readout([dict(C5, u=.78, e=.45, k="say", sev="warn", say="Out 15:20", bs="warn"), CW],
                                 name="Claude 5-hour session: warning, runs out at about 15:20, before the reset at 16:30; 78 percent used. Claude weekly: on track, 61 percent used."),
              "13:45", ("warn", "78", .78, .45)))
S.append(pair("1d &middot; Critical &mdash; countdown", "14:50. 5h: 94&nbsp;% used; empty in 12 minutes at the recent rate (reset 16:30). "
              "Under an hour the run-out becomes a <b>countdown</b>, so urgency never needs the clock. Critical fill and disc; tray: filled tile.",
              lambda th: readout([dict(C5, u=.94, e=.633, k="say", sev="crit", say="12 min left", bs="crit"), CW],
                                 name="Claude 5-hour session: critical, runs out in about 12 minutes; 94 percent used; resets at 16:30. Claude weekly: on track, 61 percent used."),
              "14:50", ("crit", "94", .94, .633)))
S.append(pair("1e &middot; Critical by level, lasts", "16:05. 5h: 92&nbsp;% used, 25 minutes to the reset, idle. Nothing will run out, but 90&nbsp;% and above is always critical: "
              "the fill turns critical and the value goes semibold; the reset stays, because it is the useful fact.",
              lambda th: readout([dict(C5, u=.92, e=.917, k="low", v="92%", t="16:30", bs="crit"), CW],
                                 name="Claude 5-hour session: critical, 92 percent used, lasts until the reset at 16:30. Claude weekly: on track, 61 percent used."),
              "16:05", ("crit", "92", .92, .917)))
S.append(pair("1f &middot; Empty", "15:10. 5h limit reached. The bar is full, the row says when it comes back: <b>Back 16:30</b>. Tray: filled tile with an empty drop.",
              lambda th: readout([dict(C5, u=1, e=.70, k="say", sev="crit", say="Back 16:30", bs="exh"), CW],
                                 name="Claude 5-hour session: empty, back at 16:30. Claude weekly: on track, 61 percent used."),
              "15:10", ("exh", "", 1, .7)))
S.append(pair("1g &middot; Just reset", "16:31. The 5-hour window reset at 16:30. Until the next reading (requested 30&ndash;90&nbsp;s after the reset) the row shows the new window "
              "as the provider defines it: 0&nbsp;%, tick at the start, next reset 21:30. No forecast for the first 15 minutes.",
              lambda th: readout([dict(C5, u=0, e=.003, v="0%", t="21:30"), CW],
                                 name="Claude 5-hour session: reset at 16:30, 0 percent used, next reset at 21:30. Claude weekly: on track, 61 percent used."),
              "16:31", ("ok", "61", .61, .74)))
S.append(pair("1h &middot; Stale &mdash; offline", "13:29. No good reading for 47 minutes (stale after 15). The last values stay, dimmed; bars turn dashed and keep the tick "
              "where it was at the reading, so the comparison stays true to that moment. The time column says why and how old.",
              lambda th: readout([dict(C5, k="stale", v="42%", st="Offline", bs="stale"), dict(CW, k="stale", v="61%", st="47m ago", bs="stale")],
                                 name="Claude: offline, last reading 47 minutes ago. Then: 5-hour session 42 percent used, weekly 61 percent used."),
              "13:29", ("stale", "61", .61, .736)))
S.append(pair("1i &middot; Stale &mdash; sign-in expired", "Same, when Claude Code's login has expired. The reason becomes the action, in the accent text colour (the only accent in the readout). "
              "A click opens the flyout at Claude's card, which explains how to renew. Lastdrop never signs in itself.",
              lambda th: readout([dict(C5, k="stale", v="42%", st="Sign in", act=True, bs="stale"), dict(CW, k="stale", v="61%", st="47m ago", bs="stale")],
                                 name="Claude: sign-in expired, last reading 47 minutes ago. Select to open Lastdrop and see how to sign in."),
              "13:29", ("stale", "61", .61, .736)))
S.append(pair("1j &middot; Starting", "First seconds after launch, before the first reading: tracks only, no tick, an en dash for the value. Tray: the Lastdrop drop.",
              lambda th: readout([dict(C5, k="none", bs="none", u=0, e=None, st="Reading&hellip;"), dict(CW, k="none", bs="none", u=0, e=None)],
                                 name="Claude: reading usage."),
              "12:40", ("start",)))

# ================================================================ 2. several services
H("2 &middot; Several services",
  "As soon as rows can come from more than one service, every row leads with the service glyph (12&nbsp;px, Lastdrop&rsquo;s own drawings, not vendor logos). "
  "The window label stays, so &ldquo;which service, which window&rdquo; is answered per row. Columns and grammar do not change. Width: <b>176&nbsp;px</b>.")
N2 = "Claude 5-hour session: on track, 42 percent used, resets at 14:56. Codex weekly: on track, 5 percent used, resets Friday 14:26."
S.append(pair("2a &middot; Two services pinned", "12:42. Claude 5-hour and Codex weekly (5&nbsp;% used, 13&nbsp;% gone, resets Friday).",
              lambda th: readout([C5, XW], multi=True, name=N2), "12:42", ("ok", "42", .42, .553), tag="176 px"))
S.append(pair("2b &middot; Two services, one in trouble", "14:50. Claude&rsquo;s 5-hour row turns critical; Codex stays calm. The glyph tells you which service without reading.",
              lambda th: readout([dict(C5, u=.94, e=.633, k="say", sev="crit", say="12 min left", bs="crit"), XW], multi=True,
                                 name="Claude 5-hour session: critical, runs out in about 12 minutes; 94 percent used. Codex weekly: on track, 5 percent used."),
              "14:50", ("crit", "94", .94, .633), tag="176 px"))
MB = dict(g="meshy", lb="Cr", u=.62, e=None, k="n", v="62%", t="")
S.append(pair("2c &middot; A balance pinned", "Claude 5-hour and Meshy credits. A balance has no window, so its bar has no tick: the fill is the share of the last top-up spent "
              "(62&nbsp;% of 3,260 credits), and the time column says how long the rest lasts at the 7-day burn rate: <b>&asymp;9d</b>. Below 7 days it turns into a warning "
              "with the same words as a window (&ldquo;Out Thu&rdquo;).",
              lambda th: readout([C5, dict(MB, k="bal")], multi=True,
                                 name="Claude 5-hour session: on track, 42 percent used, resets at 14:56. Meshy: 1,240 credits left, about 9 days at the current rate."),
              "12:42", ("ok", "61", .61, .736), tag="176 px"))
S.append(pair("2d &middot; A balance running low", "Meshy at 6 days left: caution fill, triangle, and the day it runs out.",
              lambda th: readout([C5, dict(MB, u=.81, k="say", sev="warn", say="Out Thu", bs="warn")], multi=True,
                                 name="Claude 5-hour session: on track, 42 percent used. Meshy: warning, 620 credits left, runs out around Thursday."),
              "12:42", ("warn", "81", .81, None), tag="176 px"))
S.append(pair("2e &middot; Auto, four services enabled", "12:42. Claude, Codex, Copilot and Meshy are enabled; &ldquo;auto&rdquo; shows the two most constrained meters: "
              "Copilot&rsquo;s month (on course for 88&nbsp;%) and Claude&rsquo;s week (83&nbsp;%). Rows keep the services&rsquo; order, so they do not swap as ranks change. "
              "The label column is sized for every label auto could show (here &ldquo;Fable&rdquo;), so the width never changes. The rest are in the hover card.",
              lambda th: readout([CW, PM], multi=True, lw=27,
                                 name="Auto. Claude weekly: on track, 61 percent used, resets Monday 09:00. Copilot monthly: on track, 31 percent used, resets November 1."),
              "12:42", ("ok", "31", .31, .307), tag="186 px"))
S.append(pair("2f &middot; Auto, Claude critical", "14:50. A critical meter always takes a row; Copilot keeps the other.",
              lambda th: readout([dict(C5, u=.94, e=.633, k="say", sev="crit", say="12 min left", bs="crit"), PM], multi=True, lw=27,
                                 name="Auto. Claude 5-hour session: critical, runs out in about 12 minutes. Copilot monthly: on track, 31 percent used."),
              "14:50", ("crit", "94", .94, .633), tag="186 px"))

# ================================================================ 3. space forms
H("3 &middot; Forms by available space",
  "Two rows is the default. With one meter pinned the row is centred. When the taskbar runs out of room the readout drops to the compact row "
  "(one meter, no time column), then hides; the tray icon always stays.")
S.append(pair("3a &middot; One meter pinned", "Codex weekly only: one full row, vertically centred on the clock&rsquo;s midline.",
              lambda th: readout([XW], name="Codex weekly: on track, 5 percent used, resets Friday 14:26."), "12:42", ("ok", "5", .05, .133), tag="160 px"))
for key, note, m, ic, clock in [
    ("3b &middot; Compact &mdash; on track", "The most constrained meter only: label, bar, value.", dict(CW), ("ok", "61", .61, .736), "12:42"),
    ("3c &middot; Compact &mdash; warning", "The caution triangle and a semibold value; &ldquo;Out 15:20&rdquo; moves to the hover card.", dict(C5, u=.78, e=.45, k="say", sev="warn", say="78%", bs="warn"), ("warn", "78", .78, .45), "13:45"),
    ("3d &middot; Compact &mdash; critical", "The countdown replaces the number here too.", dict(C5, u=.94, e=.633, k="say", sev="crit", say="12 min", bs="crit"), ("crit", "94", .94, .633), "14:50"),
    ("3e &middot; Compact &mdash; empty", "", dict(C5, u=1, e=.7, k="say", sev="crit", say="Empty", bs="exh"), ("exh", "", 1, .7), "15:10"),
    ("3f &middot; Compact &mdash; stale", "Dimmed value, dashed bar; the reason is in the hover card.", dict(CW, k="stale", v="61%", bs="stale"), ("stale", "61", .61, .736), "13:29"),
]:
    S.append(pair(key, note, (lambda mm: (lambda th: readout([mm], compact=True)))(m), clock, ic, tag="134 px"))
S.append(pair("3g &middot; No room", "The readout is hidden; the tray icon carries the most constrained meter alone.",
              lambda th: "", "12:42", ("ok", "61", .61, .736)))

# ================================================================ 4. environments
H("4 &middot; Transparency on, high contrast",
  "The readout never paints a background of its own (only the hover plate), and the gaps beside the tick are cut, not painted, "
  "so it sits on acrylic exactly as on a solid taskbar. In a contrast theme, fills use WindowText and Highlight; every state still reads by shape and words.")
env = ['<div class="prow">']
for th, lab in (("AL", "light, transparency on"), ("AD", "dark, transparency on")):
    env.append(f'<div class="fr wall"><span class="ttag">{lab}</span>{taskbar(th, readout([C5, CW]), icon("ok", "61", .61, .736), "12:42", w=528)}'
               f'<div style="height:8px"></div>{taskbar(th, readout([dict(C5, u=.94, e=.633, k="say", sev="crit", say="12 min left", bs="crit"), CW]), icon("crit", "94", .94, .633), "14:50", w=528)}</div>')
env.append('</div><div class="prow">')
for th, lab in (("HA", "high contrast: Aquatic"), ("HD", "high contrast: Desert")):
    env.append(f'<div class="fr"><span class="ttag">{lab}</span>{taskbar(th, readout([C5, CW]), icon("ok", "61", .61, .736), "12:42")}'
               f'<div style="height:8px"></div>{taskbar(th, readout([dict(C5, u=.78, e=.45, k="say", sev="warn", say="Out 15:20", bs="warn"), CW]), icon("warn", "78", .78, .45), "13:45")}'
               f'<div style="height:8px"></div>{taskbar(th, readout([dict(C5, u=.94, e=.633, k="say", sev="crit", say="12 min left", bs="crit"), CW]), icon("crit", "94", .94, .633), "14:50")}'
               f'<div style="height:8px"></div>{taskbar(th, readout([dict(C5, k="stale", v="42%", st="Sign in", act=True, bs="stale"), dict(CW, k="stale", v="61%", st="47m ago", bs="stale")]), icon("stale", "61", .61, .736), "13:29")}</div>')
env.append('</div>')
S.append("".join(env))


# ================================================================ 5. hover cards
def kw():
    return '<i class="kw" aria-hidden="true"></i>'


def meter_block(name, val, u, e, st, l1, r1, l2, r2, bold=False):
    return (f'<div class="cm"><div class="cr"><span class="cn">{name}</span><span class="cv{" b" if bold else ""}">{val}</span></div>'
            f'{bar(u, e, st, w=296, h=11)}'
            f'<div class="cr sub"><span>{kw()}{l1}</span><span class="{"b" if bold else ""}">{r1}</span></div>'
            f'<div class="cr sub"><span>{RESET}{l2}</span><span>{r2}</span></div></div>')


def svc_row(g, name, plan, age, st=""):
    return (f'<div class="cs-h">{glyph(g, 16)}<b>{name}</b><span class="plan">{plan}</span>'
            f'<span class="age{(" " + st) if st else ""}">{age}</span></div>')


def card(sv, title, sentence, body, extra=""):
    return (f'<div class="card" role="tooltip"><div class="ch">{sev(sv, 16)}<span class="ct">{title}</span></div>'
            f'<p class="cp">{sentence}</p>{body}{extra}</div>')


CARD_OK = card("ok", "You&rsquo;ll make it to both resets",
               "Both limits are being used more slowly than time passes. At your recent pace about 22% of the session and 17% of the week will be left when they reset.",
               svc_row("claude", "Claude", "Max", "Updated 1 min ago") +
               meter_block("5-hour session", "42% used", .42, .553, "ok", "55% of time gone", "on course for 78% at reset", "Resets 14:56", "in 2 h 14 min") +
               meter_block("Weekly &middot; all models", "61% used", .61, .736, "ok", "74% of time gone", "on course for 83% at reset", "Resets Mon 09:00", "in 1 d 20 h"))
CARD_WARN = card("warn", "5-hour limit runs out at 15:20",
                 "You&rsquo;re using it faster than time passes: 78% used with 45% of the window gone. At the pace of the last 30 minutes it runs out 1 h 10 min before the 16:30 reset. The weekly limit is fine.",
                 svc_row("claude", "Claude", "Max", "Updated 1 min ago") +
                 meter_block("5-hour session", "78% used", .78, .45, "warn", "45% of time gone", "runs out &asymp; 15:20", "Resets 16:30", "in 2 h 45 min", bold=True) +
                 meter_block("Weekly &middot; all models", "61% used", .61, .74, "ok", "74% of time gone", "on course for 83% at reset", "Resets Mon 09:00", "in 1 d 19 h"))
CARD_CRIT = card("crit", "5-hour limit runs out in 12 min",
                 "94% used. At the pace of the last 30 minutes it is empty at about 15:02, 1 h 28 min before the reset at 16:30. The weekly limit is fine.",
                 svc_row("claude", "Claude", "Max", "Updated 1 min ago") +
                 meter_block("5-hour session", "94% used", .94, .633, "crit", "63% of time gone", "runs out &asymp; 15:02", "Resets 16:30", "in 1 h 40 min", bold=True) +
                 meter_block("Weekly &middot; all models", "61% used", .61, .75, "ok", "75% of time gone", "on course for 84% at reset", "Resets Mon 09:00", "in 1 d 18 h"))
CARD_STALE = card("stale", "No new reading from Claude for 47 min",
                  "Lastdrop is offline, so these are the numbers from 12:42. Bars are dashed and frozen at that moment until a new reading arrives. Next try at 13:30.",
                  svc_row("claude", "Claude", "Max", "Last reading 12:42", "warnage") +
                  meter_block("5-hour session", "42% used", .42, .553, "stale", "55% of time gone at 12:42", "&nbsp;", "Resets 14:56", "in 1 h 27 min") +
                  meter_block("Weekly &middot; all models", "61% used", .61, .736, "stale", "74% of time gone at 12:42", "&nbsp;", "Resets Mon 09:00", "in 1 d 19 h"))


def also_row(g, name, val, reset):
    return f'<div class="ar">{glyph(g)}<span class="an">{name}</span><span class="av">{val}</span><span class="at">{reset}</span></div>'


CARD_AUTO = card("ok", "You&rsquo;ll make it to every reset",
                 "Copilot&rsquo;s month is the closest: 31% used with 31% of the month gone, on course for 88% on 1 Nov.",
                 svc_row("copilot", "Copilot", "Pro+", "Updated 2 min ago") +
                 meter_block("Monthly &middot; premium requests", "31% used", .31, .307, "ok", "31% of time gone", "on course for 88% at reset", "Resets Sun 1 Nov", "in 21 d 11 h") +
                 svc_row("claude", "Claude", "Max", "Updated 1 min ago") +
                 meter_block("Weekly &middot; all models", "61% used", .61, .736, "ok", "74% of time gone", "on course for 83% at reset", "Resets Mon 09:00", "in 1 d 20 h"),
                 '<div class="also"><div class="also-h">Also tracked</div>' +
                 also_row("claude", "Claude 5-hour", "42%", f"{RESET}14:56") +
                 also_row("claude", "Claude Fable weekly", "18%", f"{RESET}Mon") +
                 also_row("codex", "Codex weekly", "5%", f"{RESET}Fri") +
                 also_row("meshy", "Meshy credits", "1,240", "&asymp;&thinsp;9 days") + '</div>')


def stage(th, cardhtml, ro, ic, clock, h=330):
    return (f'<div class="stg" style="height:{h}px"><span class="ttag">{"light" if th == "L" else "dark"}</span>'
            f'{taskbar(th, ro.replace("<div class=\"readout\"", "<div class=\"readout hov anch\"", 1).replace("@@CARD@@", cardhtml), ic, clock)}</div>')


def ro_with_card(rows, **kw):
    return readout(rows, extra="@@CARD@@", **kw)


H("5 &middot; Hover card",
  "Hovering the readout (or the tray icon) for the system hover time opens a Windows 11 tooltip card centred above it. It follows the taskbar theme. "
  "Verdict first, then the pace in one sentence, then one block per meter with a wide copy of the same pace bar. "
  "<b>The card is the readout&rsquo;s legend:</b> the tick mark stands in front of &ldquo;% of time gone&rdquo;, and the reset glyph in front of the word &ldquo;Resets&rdquo;, every time.")
for title, crd, rows, ic, clock, h in [
    ("5a &middot; On track", CARD_OK, [C5, CW], ("ok", "61", .61, .736), "12:42", 340),
    ("5b &middot; Warning", CARD_WARN, [dict(C5, u=.78, e=.45, k="say", sev="warn", say="Out 15:20", bs="warn"), CW], ("warn", "78", .78, .45), "13:45", 356),
    ("5c &middot; Critical countdown", CARD_CRIT, [dict(C5, u=.94, e=.633, k="say", sev="crit", say="12 min left", bs="crit"), CW], ("crit", "94", .94, .633), "14:50", 340),
    ("5d &middot; Stale", CARD_STALE, [dict(C5, k="stale", v="42%", st="Offline", bs="stale"), dict(CW, k="stale", v="61%", st="47m ago", bs="stale")], ("stale", "61", .61, .736), "13:29", 340),
]:
    S.append(f'<div class="sc"><div class="sch"><b>{title}</b></div><div class="prow end">'
             + stage("L", crd, ro_with_card(rows), icon(*ic), clock, h) + stage("D", crd, ro_with_card(rows), icon(*ic), clock, h) + '</div></div>')
S.append('<div class="sc"><div class="sch"><b>5e &middot; Auto with four services</b></div>'
         '<p class="note">The card lists the meters on the readout in full, then one line for each other service&rsquo;s most constrained meter.</p><div class="prow end">'
         + stage("L", CARD_AUTO, ro_with_card([CW, PM], multi=True, lw=27), icon("ok", "31", .31, .307), "12:42", 470)
         + stage("D", CARD_AUTO, ro_with_card([CW, PM], multi=True, lw=27), icon("ok", "31", .31, .307), "12:42", 470) + '</div></div>')

# ================================================================ 6. tray icon
H("6 &middot; Tray icon &mdash; 16, 20, 24 and 32&nbsp;px",
  "Drawn at runtime for the taskbar&rsquo;s DPI from a hand-tuned layout per size (100, 125, 150, 200&nbsp;%; 175&nbsp;% uses the 24&nbsp;px layout centred, 225&nbsp;%+ the 32&nbsp;px one). "
  "The number is the most constrained meter&rsquo;s % used. Shape ladder: bare number over a pace bar (on track), outlined caution tile (warning), "
  "filled critical tile (critical), filled tile with an empty drop (empty), grey number over a dashed bar (stale), the Lastdrop drop (starting, no services). "
  "Each icon is shown at 1&times; in device pixels and at 4&times;.")
STATES = [("ok", "61", .61, .736, "on track 61"), ("ok", "8", .08, .30, "on track 8"), ("warn", "78", .78, .45, "warning 78"), ("crit", "94", .94, .633, "critical 94"),
          ("exh", "", 1, .7, "empty"), ("stale", "61", .61, .736, "stale 61"), ("start", "", 0, None, "starting")]
icons = ['<div class="iconsets">']
for th, lab in (("L", "light taskbar"), ("D", "dark taskbar")):
    icons.append(f'<div class="iset {th} {"light" if th == "L" else "dark"}"><div class="iset-h">{lab}</div><table class="itab"><tr><th></th>'
                 + "".join(f'<th>{s[4]}</th>' for s in STATES) + '</tr>')
    for size in (16, 20, 24, 32):
        icons.append(f'<tr><td class="isz">{size}&nbsp;px</td>')
        for st, n, u, e, _ in STATES:
            icons.append(f'<td><div class="icell"><div class="z4">{icon(st, n, u, e, size=size, zoom=4)}</div>'
                         f'<div class="z1">{icon(st, n, u, e, size=size, zoom=1)}</div></div></td>')
        icons.append('</tr>')
    icons.append('</table></div>')
icons.append('</div>')
S.append("".join(icons))

# ================================================================ 7. grammar + widths
H("7 &middot; Text grammar", "")
S.append(f"""<div class="gram D">
<table class="w"><tr><th>Token</th><th>Means</th><th>When</th><th>Examples</th></tr>
<tr><td><code>42%</code></td><td>share used (never &ldquo;left&rdquo;)</td><td>on track; stale (dimmed); critical by level (semibold)</td><td>0%, 5%, 61%, 99% &middot; never 100% (that is &ldquo;Back&rdquo;)</td></tr>
<tr><td><span class="kg">&#xE777;</span>&thinsp;<code>14:56</code></td><td>resets at</td><td>on track</td><td><span class="kg">&#xE777;</span>14:56 (&lt;&nbsp;12&nbsp;h) &middot; <span class="kg">&#xE777;</span>Today (same date, &ge;&nbsp;12&nbsp;h) &middot; <span class="kg">&#xE777;</span>Mon (next 6 days) &middot; <span class="kg">&#xE777;</span>22d (later)</td></tr>
<tr><td><span class="gk">{sev("warn")}</span> <code>Out 15:20</code></td><td>runs out at, before the reset</td><td>forecast empty before the reset, &ge;&nbsp;60&nbsp;min away</td><td>Out 15:20 &middot; Out Today &middot; Out Wed &middot; Out 9d</td></tr>
<tr><td><span class="gk">{sev("crit")}</span> <code>12 min left</code></td><td>runs out in</td><td>forecast empty before the reset, &lt;&nbsp;60&nbsp;min away</td><td>59 min left &hellip; 1 min left</td></tr>
<tr><td><span class="gk">{sev("crit")}</span> <code>Back 16:30</code></td><td>empty; available again at</td><td>limit reached</td><td>Back 16:30 &middot; Back Mon</td></tr>
<tr><td><code>Offline</code> / <code>Sign in</code> / <code>Paused</code> / <code>No data</code></td><td>why there is no fresh reading</td><td>stale, first row of the service</td><td>&ldquo;Sign in&rdquo; in accent text: it is an action</td></tr>
<tr><td><code>47m ago</code></td><td>age of the last good reading</td><td>stale, second row (or only row)</td><td>15m ago &middot; 3h ago &middot; 2d ago</td></tr>
<tr><td><code>&asymp;9d</code></td><td>balance lasts about</td><td>balance meter on track</td><td>&asymp;20h &middot; &asymp;9d &middot; &asymp;6w</td></tr>
</table>
<p class="intro">12/24-hour follows Windows&rsquo; short time format. The readout never shows AM/PM: a clock time appears only when it is less than 12&nbsp;hours away, so &ldquo;2:56&rdquo; can only mean the next 2:56. The hover card and the flyout show the full time.</p>
<table class="w"><tr><th>Form</th><th>Columns at 100&nbsp;% (logical px)</th><th>Width incl. 6&nbsp;px side padding</th></tr>
<tr><td>Two rows, one service</td><td>label 17 &middot; 6 &middot; bar 40 &middot; 6 &middot; value+time 79</td><td class="n">160</td></tr>
<tr><td>Two rows, several services</td><td>glyph 12 &middot; 4 &middot; label 17 &middot; 6 &middot; bar 40 &middot; 6 &middot; 79</td><td class="n">176</td></tr>
<tr><td>Auto incl. &ldquo;Fable&rdquo; label</td><td>glyph 12 &middot; 4 &middot; label 27 &middot; 6 &middot; bar 40 &middot; 6 &middot; 79</td><td class="n">186</td></tr>
<tr><td>One meter pinned</td><td>as two rows, one row centred</td><td class="n">160</td></tr>
<tr><td>Compact (one service / several)</td><td>label 17 &middot; 6 &middot; bar 40 &middot; 6 &middot; cell 53 (+16 with glyphs)</td><td class="n">134 / 150</td></tr>
</table>
<p class="intro">Widths are computed once per locale and pinned set from the widest token that can occur (see readout-spec &sect;6), never from the current data, so the readout keeps one width while its state changes.</p>
</div>""")

# ================================================================ page
LEGEND = f"""<div class="legend L light">
<div class="lg"><canvas class="pb" data-w="96" data-h="16" data-u=".42" data-e=".553" data-s="ok"></canvas><span>fill = <b>share used</b> &middot; tick = <b>share of the window gone</b>. Fill short of the tick: you are under pace.</span></div>
<div class="lg"><span class="lgk">{sev("warn")}<canvas class="pb" data-w="40" data-h="16" data-u=".78" data-e=".45" data-s="warn"></canvas></span><span><b>Out 15:20</b> &mdash; at the recent rate it runs out before the reset</span></div>
<div class="lg"><span class="lgk">{sev("crit")}<canvas class="pb" data-w="40" data-h="16" data-u=".94" data-e=".633" data-s="crit"></canvas></span><span><b>12 min left</b> &mdash; runs out within the hour (critical at 30&nbsp;min or less, and at 90&nbsp;% used)</span></div>
<div class="lg"><span class="lgk"><i class="rg" style="font-size:14px;width:14px">&#xE777;</i></span><span><b>resets at</b> (taught by the hover card: &ldquo;<i class="rg">&#xE777;</i>&thinsp;Resets 14:56&rdquo;)</span></div>
</div>"""

PAGE_CSS = r"""
body{background:#5f636b;padding:24px 24px 40px}
h1{font:600 22px/28px var(--font);color:#fff;margin:0 0 6px}
h2{font:600 16px/22px var(--font);color:#fff;margin:34px 0 4px}
.intro{font:13px/19px var(--font);color:#eef0f3;max-width:1110px;margin:2px 0 10px}
.intro b{color:#fff}
.lead{font:14px/21px var(--font);color:#f4f4f4;max-width:1110px;margin:0 0 12px}
.sc{margin:14px 0 6px}
.sch{font:600 13px/18px var(--font);color:#fff}
.sch b{font-weight:600}
.wtag{font:400 11px var(--font);color:#e6e6e6;margin-left:8px;padding:1px 6px;border:1px solid rgba(255,255,255,.4);border-radius:4px}
p.note{font:12px/17px var(--font);color:#e2e4e8;margin:2px 0 6px;max-width:1110px}
.prow{display:flex;gap:16px;align-items:flex-start}
.prow.end{align-items:flex-end}
.fr{position:relative;padding-top:14px}
.fr.full{margin-bottom:8px}
.ttag{position:absolute;left:0;top:0;font:11px/12px var(--font);color:#d9dbe0}
.taskbar{margin:0}
.taskbar .ld-icon canvas{display:block}
.fr.wall{padding:22px 12px 12px;border-radius:8px;background:linear-gradient(120deg,#3b6bd6 0%,#8b52c9 35%,#e0697a 65%,#f1b25a 100%)}
.fr.wall .ttag{left:12px;top:5px;color:#fff}
.taskbar.acr.light{background:rgba(243,243,243,.70);backdrop-filter:blur(30px) saturate(1.4);border-top-color:rgba(0,0,0,.08)}
.taskbar.acr.dark{background:rgba(28,28,28,.72);backdrop-filter:blur(30px) saturate(1.4);border-top-color:rgba(255,255,255,.08)}
.taskbar.hca{background:#202020;border-top:1px solid #fff;--tb-fg:#fff;--tb-fg2:#A6A6A6}
.taskbar.hcd{background:#FFFAEF;border-top:1px solid #3D3D3D;--tb-fg:#3D3D3D;--tb-fg2:#676767}
.taskbar.hca .readout.hov,.taskbar.hcd .readout.hov{outline:1px solid var(--acc)}

/* ---------- hover card ---------- */
.stg{position:relative;display:flex;flex-direction:column;justify-content:flex-end;padding-top:14px}
.card{position:absolute;bottom:calc(100% + 12px);left:50%;transform:translateX(-50%);width:320px;padding:12px 12px 12px;box-sizing:border-box;
  background:var(--card-bg);border:1px solid var(--card-stroke);border-radius:8px;box-shadow:var(--card-shadow);
  color:var(--fg);font:400 12px/16px "Segoe UI Variable Small","Segoe UI",sans-serif;font-variant-numeric:tabular-nums;white-space:normal;text-align:left;cursor:default}
.card .ch{display:flex;gap:8px;align-items:flex-start}
.card .ct{font:600 14px/20px "Segoe UI Variable Text","Segoe UI",sans-serif}
.card .cp{margin:4px 0 0}
.card .cs-h{display:flex;align-items:center;gap:6px;margin-top:12px;padding-top:10px;border-top:1px solid var(--div);height:16px;box-sizing:content-box}
.card .cs-h svg{width:16px;height:16px}
.card .cs-h b{font-weight:600}
.card .plan{color:var(--fg2)}
.card .age{margin-left:auto;color:var(--fg2)}
.card .cm{margin-top:10px}
.card .cr{display:flex;justify-content:space-between;align-items:baseline;gap:8px;white-space:nowrap}
.card .cn{font-weight:600}
.card .cr .b,.card .cv.b{font-weight:600}
.card .sub{color:var(--fg2)}
.card .sub .b{color:var(--fg)}
.card canvas.pb{margin:1px 0 1px}
.kw{display:inline-block;width:1px;height:9px;background:var(--fg);margin:0 6px 0 3px;vertical-align:-1px}
.card .sub .rg{margin-right:3px}
.also{margin-top:12px;padding-top:10px;border-top:1px solid var(--div)}
.also-h{color:var(--fg2);margin-bottom:4px}
.ar{display:grid;grid-template-columns:12px 1fr auto 56px;gap:6px;align-items:center;height:20px}
.ar .av{text-align:right}
.ar .at{display:flex;align-items:center;justify-content:flex-start;color:var(--fg)}

/* ---------- tray icon panel ---------- */
.iconsets{display:flex;flex-direction:column;gap:14px}
.iset{border-radius:8px;padding:10px 14px 12px}
.iset.L{background:#eeeeee} .iset.D{background:#1c1c1c}
.iset-h{font:600 12px var(--font);color:var(--fg2);margin-bottom:6px}
.itab{border-collapse:collapse}
.itab th{font:400 11px/14px var(--font);color:var(--fg2);text-align:center;padding:0 6px 6px;min-width:134px}
.itab td{padding:4px 6px;vertical-align:bottom;text-align:center}
.itab td.isz{font:600 11px var(--font);color:var(--fg2);text-align:left;min-width:44px;padding-right:10px}
.icell{display:flex;flex-direction:column;align-items:center;justify-content:flex-end;gap:6px}
.z4 canvas{image-rendering:pixelated;display:block;outline:1px dashed rgba(127,127,127,.35)}
.z1 canvas{display:block}

/* ---------- legend / tables ---------- */
.legend{display:flex;flex-wrap:wrap;gap:8px 28px;padding:12px 16px;border-radius:8px;background:#eeeeee;color:#1a1a1a;
  font:12px/16px "Segoe UI Variable Small",sans-serif;max-width:1078px;margin:6px 0 4px}
.lg{display:flex;align-items:center;gap:10px}
.lgk{display:flex;align-items:center;gap:6px}
table.w{border-collapse:collapse;font:12px/16px var(--font);color:#f2f2f2;font-variant-numeric:tabular-nums;margin:6px 0 10px}
table.w th,table.w td{padding:5px 14px 5px 0;text-align:left;border-bottom:1px solid rgba(255,255,255,.18);vertical-align:top}
table.w th{font-weight:600;color:#fff}
table.w td.n{text-align:right;padding-right:6px}
table.w code{font:600 12px "Segoe UI Variable Small";color:#fff}
.kg{font:normal 12px "Segoe Fluent Icons";color:#fff}
.gk{display:inline-flex;vertical-align:-3px}
.ro .rg.ap{font:400 12px/16px "Segoe UI Variable Small";width:auto;margin-right:1px;color:var(--fg2)}
"""

html = f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Lastdrop readout, final</title>
<link rel="stylesheet" href="harness.css">
<style>{TOKENS_CSS}{PAGE_CSS}</style>
</head>
<body>
<h1>Lastdrop &mdash; taskbar readout, final design</h1>
<p class="lead">Concept A &ldquo;Pace bars&rdquo; with two grafts: from C, a <b>countdown</b> (&ldquo;12 min left&rdquo;) whenever the run-out is less than an hour away; from D, the <b>service glyph</b> at the head of every row once rows can come from more than one service.
The readout answers &ldquo;will I make it to the reset?&rdquo; with one comparison: where the fill stops against the tick. It stays in the taskbar&rsquo;s own monochrome text colour while the answer is yes; caution or critical colour, a severity glyph and words appear together, and only when the answer is no.
Everything here is drawn at 1:1 (100&nbsp;%) or, in readout-150.png, at 150&nbsp;% with the bars and icons computed in device pixels by the same reference code the native renderer follows (see the &lt;script&gt; at the end of this file).</p>
{LEGEND}
{"".join(hero)}
{"".join(S)}
<script>{DRAW_JS}</script>
</body>
</html>
"""
open(OUT, "w", encoding="utf-8").write(html)
print("ok", len(html))
