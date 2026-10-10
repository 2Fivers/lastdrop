// Writes icons/icon.ico (32x32, 32-bit BMP in ICO) - a ring gauge at 42%, Windows default blue.
// Only used as the exe resource icon required by tauri-build; the tray icon is drawn at runtime in Rust.
import fs from 'node:fs';
const N = 32, SS = 4, C = N / 2, RMID = 12, HW = 3, PCT = 0.42;
const FG = [0x00, 0x78, 0xd4], TRACK = [128, 128, 128, 0.45];
const sweep = PCT * Math.PI * 2;
const px = new Uint8Array(N * N * 4);
const endX = RMID * Math.sin(sweep), endY = -RMID * Math.cos(sweep);
for (let y = 0; y < N; y++) for (let x = 0; x < N; x++) {
  let a = 0, t = 0;
  for (let sy = 0; sy < SS; sy++) for (let sx = 0; sx < SS; sx++) {
    const X = x + (sx + 0.5) / SS - C, Y = y + (sy + 0.5) / SS - C;
    const d = Math.hypot(X, Y);
    const band = Math.abs(d - RMID) <= HW;
    let ang = Math.atan2(X, -Y); if (ang < 0) ang += Math.PI * 2;
    const arc = (band && ang <= sweep) || Math.hypot(X, Y + RMID) <= HW || Math.hypot(X - endX, Y - endY) <= HW;
    if (arc) a++; else if (band) t++;
  }
  a /= SS * SS; t /= SS * SS;
  const ta = t * TRACK[3], al = a + ta * (1 - a);
  const i = (y * N + x) * 4;
  if (al > 0) for (let k = 0; k < 3; k++) px[i + k] = Math.round((FG[k] * a + TRACK[k] * ta * (1 - a)) / al);
  px[i + 3] = Math.round(al * 255);
}
const xorSize = N * N * 4, andSize = N * 4, bmpSize = 40 + xorSize + andSize;
const buf = Buffer.alloc(6 + 16 + bmpSize);
buf.writeUInt16LE(0, 0); buf.writeUInt16LE(1, 2); buf.writeUInt16LE(1, 4);
buf.writeUInt8(N, 6); buf.writeUInt8(N, 7); buf.writeUInt8(0, 8); buf.writeUInt8(0, 9);
buf.writeUInt16LE(1, 10); buf.writeUInt16LE(32, 12); buf.writeUInt32LE(bmpSize, 14); buf.writeUInt32LE(22, 18);
let o = 22;
buf.writeUInt32LE(40, o); buf.writeInt32LE(N, o + 4); buf.writeInt32LE(N * 2, o + 8);
buf.writeUInt16LE(1, o + 12); buf.writeUInt16LE(32, o + 14); buf.writeUInt32LE(0, o + 16);
buf.writeUInt32LE(xorSize + andSize, o + 20);
o += 40;
for (let y = N - 1; y >= 0; y--) for (let x = 0; x < N; x++) {
  const i = (y * N + x) * 4;
  buf[o++] = px[i + 2]; buf[o++] = px[i + 1]; buf[o++] = px[i]; buf[o++] = px[i + 3];
}
// AND mask all zero (alpha channel is used)
fs.mkdirSync('icons', { recursive: true });
fs.writeFileSync('icons/icon.ico', buf);
console.log('icons/icon.ico', buf.length, 'bytes');
