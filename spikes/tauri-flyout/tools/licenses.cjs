// Aggregate licences of the crates actually in the Windows release dependency graph (normal deps only).
const m = require(require('path').join(__dirname, '..', 'target', 'meta.json'));
const byId = new Map(m.packages.map((p) => [p.id, p]));
const nodes = new Map(m.resolve.nodes.map((n) => [n.id, n]));
const rootId = m.resolve.root;
const seen = new Set(); const stack = [rootId];
while (stack.length) {
  const id = stack.pop(); if (seen.has(id)) continue; seen.add(id);
  for (const d of nodes.get(id).deps) if (d.dep_kinds.some((k) => k.kind === null)) stack.push(d.pkg);
}
const lic = {};
for (const id of seen) { if (id === rootId) continue; const p = byId.get(id); const l = p.license || ('FILE:' + p.license_file); (lic[l] ||= []).push(p.name + '@' + p.version); }
console.log('runtime crates:', seen.size - 1);
for (const [l, ps] of Object.entries(lic).sort((a, b) => b[1].length - a[1].length))
  console.log(String(ps.length).padStart(4), l, ps.length <= 6 ? ' <- ' + ps.join(', ') : '');
