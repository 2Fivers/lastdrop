"""Reference model used to compute the test vectors printed in readout-spec.md. Not shipped."""
import math
from datetime import datetime, timedelta

TOL = 0.03


def round_half_up(x):
    return math.floor(x + 0.5)


def elapsed_share(now, R, L):
    return min(1.0, max(0.0, 1 - (R - now) / L))


def display_pct(u, exhausted):
    p = round_half_up(u * 100)
    lo = 1 if u > 0 else 0
    hi = 100 if exhausted else 99
    return max(lo, min(hi, p))


def odd_near(target, hair):
    return 2 * round_half_up((target - hair) / 2) + hair


def pace_geometry(W, s, u, e, state="ok"):
    L = round_half_up(W * s)
    hair = max(1, math.floor(s))
    fillH = odd_near(3 * s, hair)
    tickH = odd_near(9 * s, hair)
    f = round_half_up(min(1, max(0, u)) * L)
    if u > 0 and f == 0:
        f = 1
    if state == "exh":
        f = L
    k = None if e is None else min(L - hair, max(0, round_half_up(e * L)))
    if k is not None and state != "exh":
        if u < e - TOL and f > k - hair - 1:
            f = max(1 if u > 0 else 0, k - hair - 1)
        if u > e + TOL and f < k + 2 * hair + 1:
            f = min(L, k + 2 * hair + 1)
    cuts = [] if k is None else [(k - hair, k), (k + hair, k + 2 * hair)]

    def minus(a, b):
        parts = [(a, b)]
        for c0, c1 in cuts:
            nxt = []
            for p, q in parts:
                if c0 > p:
                    nxt.append((p, min(q, c0)))
                if c1 < q:
                    nxt.append((max(p, c1), q))
            parts = [(x, y) for x, y in nxt if y > x]
        return parts
    return dict(L=L, hair=hair, fillH=fillH, tickH=tickH, fill=f, tick=k, fill_segs=minus(0, f), track_segs=minus(f, L))


WD = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]


def time_token(now, t, h24=True):
    d = t - now
    if d < timedelta(hours=12):
        if h24:
            return t.strftime("%H:%M")
        h = t.hour % 12 or 12
        return f"{h}:{t.minute:02d}"
    days = (t.date() - now.date()).days
    if days == 0:
        return "Today"
    if days <= 6:
        return WD[t.weekday()]
    return f"{days}d"


def rel_token(now, t):
    d = (t - now).total_seconds()
    if d < 3600:
        return f"{max(1, math.ceil(d / 60))}m"
    if d < 86400:
        return f"{int(d // 3600)}h"
    return f"{int(d // 86400)}d"


def countdown(now, t_out):
    n = max(1, math.ceil((t_out - now).total_seconds() / 60))
    return f"{n} min left"


def age_token(now, t):
    s = (now - t).total_seconds()
    if s < 3600:
        return f"{int(s // 60)}m ago"
    if s < 172800:
        return f"{int(s // 3600)}h ago"
    return f"{int(s // 86400)}d ago"


def forecast(readings, R, L, now_reading_t):
    """readings: list of (t, u) in the current window, ascending. Returns (rate_per_h, p, t_out) or None."""
    S = min(timedelta(hours=24), max(timedelta(minutes=30), L / 10))
    tn, un = readings[-1]
    window_start = R - L
    if tn - window_start < timedelta(minutes=15):
        return None
    older = [r for r in readings if r[0] <= tn - S]
    a = older[-1] if older else readings[0]
    span = tn - a[0]
    if span < timedelta(minutes=15):
        return None
    rate = max(0.0, (un - a[1]) / (span.total_seconds() / 3600))
    p = un + rate * ((R - tn).total_seconds() / 3600)
    t_out = None
    if rate > 0 and un < 1:
        t_out = tn + timedelta(hours=(1 - un) / rate)
    return rate, p, t_out


if __name__ == "__main__":
    now = datetime(2026, 10, 10, 12, 42)
    print("e 5h", elapsed_share(now, datetime(2026, 10, 10, 14, 56), timedelta(hours=5)))
    print("e wk", elapsed_share(now, datetime(2026, 10, 12, 9, 0), timedelta(days=7)))
    print("e codex", elapsed_share(now, datetime(2026, 10, 16, 14, 26), timedelta(days=7)))
    for u, ex in [(0, False), (0.004, False), (0.4249, False), (0.425, False), (0.8949, False), (0.895, False), (0.996, False), (1.0, True), (0.97, True)]:
        print("pct", u, ex, display_pct(u, ex))
    for s in (1, 1.25, 1.5, 1.75, 2, 2.5):
        print("geom", s, pace_geometry(40, s, .42, .5533))
    print("geom ahead", pace_geometry(40, 1, .68, .63))
    print("geom on", pace_geometry(40, 1, .55, .5533))
    print("geom empty start", pace_geometry(40, 1, 0, 0.003))
    print("geom exh", pace_geometry(40, 1, 1, 0.70, "exh"))
    print("geom tiny", pace_geometry(40, 1, 0.004, 0.30))
    print("geom e1", pace_geometry(40, 1, 0.5, 1.0))
    for t in [datetime(2026, 10, 10, 14, 56), datetime(2026, 10, 11, 0, 30), datetime(2026, 10, 11, 1, 0), datetime(2026, 10, 12, 9, 0),
              datetime(2026, 10, 16, 14, 26), datetime(2026, 10, 17, 9, 0), datetime(2026, 11, 1, 0, 0)]:
        print("tok", t, time_token(now, t), time_token(now, t, False), rel_token(now, t))
    n2 = datetime(2026, 10, 10, 0, 30)
    print("tok today", time_token(n2, datetime(2026, 10, 10, 18, 0)), time_token(n2, datetime(2026, 10, 10, 18, 0), False))
    print("tok 12h", time_token(datetime(2026, 10, 10, 13, 0), datetime(2026, 10, 11, 0, 59), False))
    n3 = datetime(2026, 10, 10, 14, 50)
    for m in (12, 11.5, 0.3, 59.2, 60):
        print("cd", m, countdown(n3, n3 + timedelta(minutes=m)))
    for m in (15, 47, 59.9, 60, 61, 47 * 60, 49 * 60):
        print("age", m, age_token(n3, n3 - timedelta(minutes=m)))
    # forecasts
    L5 = timedelta(hours=5)
    R = datetime(2026, 10, 10, 16, 30)
    rd = [(datetime(2026, 10, 10, 13, 10), .70), (datetime(2026, 10, 10, 13, 15), .715), (datetime(2026, 10, 10, 13, 40), .775), (datetime(2026, 10, 10, 13, 45), .78)]
    print("fc warn", forecast(rd, R, L5, rd[-1][0]))
    rd2 = [(datetime(2026, 10, 10, 14, 15), .79), (datetime(2026, 10, 10, 14, 20), .815), (datetime(2026, 10, 10, 14, 45), .915), (datetime(2026, 10, 10, 14, 50), .94)]
    print("fc crit", forecast(rd2, R, L5, rd2[-1][0]))
    R1 = datetime(2026, 10, 10, 14, 56)
    rd3 = [(datetime(2026, 10, 10, 12, 7), .36), (datetime(2026, 10, 10, 12, 12), .375), (datetime(2026, 10, 10, 12, 42), .42)]
    print("fc ok", forecast(rd3, R1, L5, rd3[-1][0]))
    rd4 = [(datetime(2026, 10, 10, 16, 31), 0.0), (datetime(2026, 10, 10, 16, 41), .06)]
    print("fc guard", forecast(rd4, datetime(2026, 10, 10, 21, 30), L5, rd4[-1][0]))
    rd5 = [(datetime(2026, 10, 10, 16, 31), 0.0), (datetime(2026, 10, 10, 16, 46), .08)]
    print("fc 15min", forecast(rd5, datetime(2026, 10, 10, 21, 30), L5, rd5[-1][0]))
    rd6 = [(datetime(2026, 10, 10, 15, 30), .92), (datetime(2026, 10, 10, 16, 5), .92)]
    print("fc idle", forecast(rd6, R, L5, rd6[-1][0]))
