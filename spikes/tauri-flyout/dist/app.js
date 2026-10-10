// Lastdrop flyout - vanilla JS, no framework, no bundler.
'use strict';
(function () {
  const INIT = window.__LASTDROP_INIT || {};
  const root = document.documentElement;
  const invoke = (cmd, args) =>
    window.__TAURI__ && window.__TAURI__.core
      ? window.__TAURI__.core.invoke(cmd, args)
      : Promise.resolve(null);

  function setPalette(p) {
    if (!p) return;
    for (const k in p) root.style.setProperty('--' + k, p[k]);
  }
  function setTheme(t) {
    if (t === 'light' || t === 'dark') root.dataset.theme = t;
    else delete root.dataset.theme;
  }
  setPalette(INIT.palette);
  setTheme(INIT.theme);

  // ---------------- mock data (static) ----------------
  const meshy = (() => {
    // 30 points descending 2400 -> 1240 with small deterministic noise
    let seed = 7;
    const rnd = () => ((seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
    const pts = [];
    for (let i = 0; i < 30; i++) {
      const base = 2400 - (1160 * i) / 29;
      const noise = i === 0 || i === 29 ? 0 : (rnd() - 0.5) * 70;
      pts.push(Math.round(base + noise));
    }
    return pts;
  })();

  const DATA = [
    {
      name: 'Claude', plan: 'Max',
      pace: 'On pace — lasts until reset',
      meters: [
        { label: '5-hour session', pct: 42, meta: 'Resets in 2 h 14 min' },
        { label: 'Weekly · all models', pct: 61, meta: 'Resets Thu 09:00' },
        { label: 'Weekly · Fable', pct: 18, meta: '' },
      ],
    },
    {
      name: 'Codex', plan: 'Pro Lite',
      meters: [{ label: 'Weekly', pct: 5, meta: 'Resets Fri 14:26' }],
      spark: [3, 5, 4, 8, 12, 9, 15, 18, 14, 20, 26, 22, 30, 5],
    },
    {
      name: 'Meshy',
      big: '1,240', bigUnit: 'credits',
      spark: meshy,
      caption: '≈ 9 days left at current pace',
    },
  ];

  // ---------------- tiny DOM helpers ----------------
  const SVGNS = 'http://www.w3.org/2000/svg';
  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }
  function s(tag, attrs) {
    const el = document.createElementNS(SVGNS, tag);
    for (const k in attrs) el.setAttribute(k, attrs[k]);
    return el;
  }

  function meter(m) {
    const wrap = h('div', 'meter');
    const line = h('div', 'meter-line');
    line.append(h('span', 'lbl', m.label), h('span', 'meta', m.meta || ''), h('span', 'pct', m.pct + '%'));
    const bar = h('div', 'bar');
    bar.setAttribute('role', 'progressbar');
    bar.setAttribute('aria-valuenow', String(m.pct));
    bar.setAttribute('aria-label', m.label);
    const fill = h('div', 'fill');
    fill.style.setProperty('--v', m.pct + '%');
    bar.append(fill);
    wrap.append(line, bar);
    return wrap;
  }

  // Sparkline drawn in real pixel space so the stroke and end dot stay crisp.
  function sparkline(values, width, H) {
    const pad = 3;
    const w = Math.max(10, width);
    const min = Math.min(0, ...values), max = Math.max(...values);
    const span = max - min || 1;
    const x = (i) => (i * (w - pad * 2)) / (values.length - 1) + pad;
    const y = (v) => H - pad - ((v - min) * (H - pad * 2)) / span;
    let d = '';
    values.forEach((v, i) => { d += (i ? 'L' : 'M') + x(i).toFixed(2) + ' ' + y(v).toFixed(2); });
    const area = d + 'L' + x(values.length - 1).toFixed(2) + ' ' + (H - pad) + 'L' + x(0).toFixed(2) + ' ' + (H - pad) + 'Z';
    const svg = s('svg', { class: 'spark', viewBox: `0 0 ${w} ${H}`, width: w, height: H, 'aria-hidden': 'true' });
    svg.style.height = H + 'px';
    svg.append(
      s('line', { class: 'base', x1: pad, x2: w - pad, y1: H - pad + 0.5, y2: H - pad + 0.5 }),
      s('path', { class: 'area', d: area }),
      s('path', { class: 'line', d }),
      s('circle', { class: 'end', cx: x(values.length - 1), cy: y(values[values.length - 1]), r: 2.5 }),
    );
    return svg;
  }

  function render() {
    const content = document.getElementById('content');
    const frag = document.createDocumentFragment();
    const sparks = [];
    for (const svc of DATA) {
      const card = h('section', 'card');
      const hd = h('div', 'card-hd');
      hd.append(h('span', 'card-title', svc.name));
      if (svc.plan) hd.append(h('span', 'badge', svc.plan));
      hd.append(h('span', 'spacer'));
      if (svc.pace) {
        const p = h('span', 'pace');
        p.append(h('span', 'dot'), document.createTextNode(svc.pace));
        hd.append(p);
      }
      card.append(hd);
      (svc.meters || []).forEach((m) => card.append(meter(m)));
      if (svc.big) {
        // stat on the left (big number + caption), trend sparkline on the right
        const split = h('div', 'split');
        const stat = h('div', 'stat');
        const num = h('div', 'big-row');
        num.append(h('span', 'big', svc.big), h('span', 'big-unit', svc.bigUnit));
        stat.append(num);
        if (svc.caption) stat.append(h('div', 'caption', svc.caption));
        const holder = h('div', 'spark-holder');
        split.append(stat, holder);
        card.append(split);
        sparks.push([holder, svc.spark, 40]);
      } else if (svc.spark) {
        const holder = h('div', 'spark-holder');
        card.append(holder);
        sparks.push([holder, svc.spark, 32]);
      }
      frag.append(card);
    }
    content.append(frag);
    // one layout read, then draw sparklines at the real width
    const draw = () => {
      for (const [holder, vals, ht] of sparks) {
        const w = holder.clientWidth || 150;
        if (holder.dataset.w === String(w)) continue;
        holder.dataset.w = String(w);
        holder.replaceChildren(sparkline(vals, w, ht));
      }
    };
    draw();
    window.addEventListener('resize', draw);
  }
  render();

  // ---------------- interactions ----------------
  const refresh = document.getElementById('refresh');
  refresh.addEventListener('click', () => {
    refresh.classList.remove('spin');
    void refresh.offsetWidth;
    refresh.classList.add('spin');
    document.getElementById('updated').textContent = 'Updated just now';
  });
  document.getElementById('settings').addEventListener('click', () => invoke('open_settings'));
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape') invoke('hide_flyout'); });
  document.addEventListener('contextmenu', (e) => e.preventDefault());

  // ---------------- first-frame measurement ----------------
  // Rust calls __lastdrop.measure(seq) right after showing the window. We wait until the
  // page is visible, then two requestAnimationFrame callbacks (the second one runs after
  // the first frame with content has been produced), then report over IPC.
  function measure(seq) {
    const t0 = performance.now();
    let tVisible = null;
    const go = () => {
      tVisible = performance.now();
      requestAnimationFrame(() => requestAnimationFrame(() => {
        const t2 = performance.now();
        invoke('frame_presented', { seq, jsTotalMs: t2 - t0, jsVisibleToFrameMs: t2 - tVisible });
      }));
    };
    if (document.visibilityState === 'visible') go();
    else {
      const onVis = () => {
        if (document.visibilityState === 'visible') {
          document.removeEventListener('visibilitychange', onVis);
          go();
        }
      };
      document.addEventListener('visibilitychange', onVis);
    }
  }

  window.__lastdrop = { measure, setPalette, setTheme };
  invoke('page_ready', {
    visibility: document.visibilityState,
    viewport: innerWidth + 'x' + innerHeight + '@' + devicePixelRatio + ' overflowPx=' +
      (document.getElementById('content').scrollHeight - document.getElementById('content').clientHeight),
  });
})();
