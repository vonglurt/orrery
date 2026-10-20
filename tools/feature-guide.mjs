// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson
//
// THE FEATURE GUIDE'S PICTURES, TAKEN. docs/FEATURE_GUIDE.md is a tour of the
// console in pictures; this is what took them, and running it again retakes
// every one from the binary as it stands.
//
//   cargo build --release && node tools/feature-guide.mjs   -> docs/features/*.png
//
// No fleet is involved and none can appear in a picture:
//   * the wall runs with --demo, which is the museum fixture in src/fleet.rs;
//   * the museum interface is drawn by --frame against the same fixture, the
//     only way to see it without a compositor (README: "Every screenshot in
//     docs/ is a file this produced");
//   * the seat dials a SIMULATED node: a forty-line RFB server below, on
//     127.0.0.1, that draws a made-up exhibit screen, found through a
//     made-up `copal fleet` (this file again, with --fleet-state) whose
//     document is the fixture with museum-05's address set to 127.0.0.1.
//   * every orrery runs with HOME set to an empty folder, so no answers.txt
//     and no operator key is ever read.
//
// Ports 8180-8183. Needs `chromium` and util-linux `script` on PATH, and node
// 22 or later (global WebSocket). Zero dependencies.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import zlib from 'node:zlib';

// ── the made-up `copal fleet`: print the saved document, whatever was asked ──
if (process.argv[2] === '--fleet-state') {
  process.stdout.write(fs.readFileSync(process.argv[3], 'utf8'));
  process.exit(0);
}

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const BIN = path.join(ROOT, 'target/release/orrery');
const SHOTS = path.join(ROOT, 'docs/features');
// A fixed path, because the Exchange frame prints this folder's name.
const BOX = '/tmp/orrery-feature-guide';
const GALLERY = 8180, OPERATOR = 8181, VNC = 8182, CDP = 8183;
const TOKEN = 'demo-token';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const T0 = Date.now();
const say = (...a) => console.log(`[${((Date.now() - T0) / 1000).toFixed(1)}s]`, ...a);

const children = [];
const stop = () => { for (const c of children) { try { c.kill(); } catch {} } };
process.on('SIGINT', () => { stop(); process.exit(130); });

function run(args, opts = {}) {
  return new Promise((res, rej) => {
    const c = spawn(BIN, args, { env: { ...process.env, HOME: path.join(BOX, 'home'), ...(opts.env || {}) }, cwd: opts.cwd || BOX, stdio: ['ignore', 'pipe', 'pipe'] });
    let err = '';
    c.stderr.on('data', (d) => (err += d));
    c.on('exit', (code) => (code === 0 ? res(err) : rej(new Error(`orrery ${args.join(' ')}: ${err}`))));
  });
}
function serve(args) {
  const c = spawn(BIN, args, { env: { ...process.env, HOME: path.join(BOX, 'home') }, cwd: BOX, stdio: 'ignore' });
  children.push(c);
  return c;
}

// ── PPM -> PNG, by hand (zlib is in node) ────────────────────────────────────
const CRC = new Int32Array(256).map((_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c; });
const crc32 = (b) => { let c = -1; for (const x of b) c = CRC[(c ^ x) & 255] ^ (c >>> 8); return (c ^ -1) >>> 0; };
function chunk(type, data) {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0); out.write(type, 4, 'ascii'); data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
}
function ppmToPng(ppm) {
  // P6 header: magic, width, height, maxval, each separated by whitespace.
  const head = []; let i = 0;
  while (head.length < 4) { while (/\s/.test(String.fromCharCode(ppm[i]))) i++; let s = ''; while (!/\s/.test(String.fromCharCode(ppm[i]))) s += String.fromCharCode(ppm[i++]); head.push(s); }
  i++;
  const [magic, w, h] = [head[0], Number(head[1]), Number(head[2])];
  if (magic !== 'P6') throw new Error('not a P6 PPM');
  const raw = Buffer.alloc((w * 3 + 1) * h);
  for (let y = 0; y < h; y++) ppm.copy(raw, y * (w * 3 + 1) + 1, i + y * w * 3, i + (y + 1) * w * 3);
  const ihdr = Buffer.alloc(13); ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = 8; ihdr[9] = 2;
  return { w, h, png: Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw, { level: 9 })), chunk('IEND', Buffer.alloc(0))]) };
}

// ── the simulated node's screen, over RFB 3.8, security type None ────────────
const FW = 800, FH = 450;
const clicks = [];
let drawn = null, drawnClicks = -1;
function exhibit() {
  if (drawnClicks === clicks.length) return drawn;
  // A made-up exhibit: an orrery on a night-blue ground. Four bytes a pixel,
  // [pad, R, G, B], which is what the seat asks for in set_pixel_format.
  const px = Buffer.alloc(FW * FH * 4);
  const put = (x, y, r, g, b) => { if (x < 0 || y < 0 || x >= FW || y >= FH) return; const o = (y * FW + x) * 4; px[o + 1] = r; px[o + 2] = g; px[o + 3] = b; };
  for (let y = 0; y < FH; y++) for (let x = 0; x < FW; x++) put(x, y, 10 + y / 18, 18 + y / 12, 40 + y / 8);
  const cx = 400, cy = 225;
  const disc = (x0, y0, r, c) => { for (let y = -r; y <= r; y++) for (let x = -r; x <= r; x++) if (x * x + y * y <= r * r) put(Math.round(x0 + x), Math.round(y0 + y), ...c); };
  const ring = (rx, ry, c) => { for (let a = 0; a < 2 * Math.PI; a += 0.002) for (let t = -2; t <= 2; t++) put(Math.round(cx + (rx + t) * Math.cos(a)), Math.round(cy + (ry + t) * Math.sin(a)), ...c); };
  const orbits = [[80, 40, [230, 170, 90], 10, 0.6], [140, 70, [120, 190, 230], 14, 2.2], [210, 105, [200, 110, 80], 13, 4.0], [290, 145, [220, 200, 150], 22, 5.3], [370, 185, [150, 200, 190], 17, 0.9]];
  for (const [rx, ry] of orbits) ring(rx, ry, [70, 90, 130]);
  disc(cx, cy, 30, [255, 200, 90]); disc(cx, cy, 22, [255, 230, 150]);
  for (const [rx, ry, c, r, a] of orbits) disc(cx + rx * Math.cos(a), cy + ry * Math.sin(a), r, c);
  for (let s = 0; s < 160; s++) put((s * 97) % FW, (s * 53) % FH, 230, 230, 255);
  for (const [x, y] of clicks) { for (let k = 0; k < 3; k++) for (let a = 0; a < 2 * Math.PI; a += 0.01) put(Math.round(x + (16 + k) * Math.cos(a)), Math.round(y + (16 + k) * Math.sin(a)), 255, 80, 60); disc(x, y, 4, [255, 80, 60]); }
  drawn = px; drawnClicks = clicks.length;
  return px;
}
function vncServer() {
  const srv = net.createServer((s) => {
    let buf = Buffer.alloc(0), stage = 'version';
    s.write('RFB 003.008\n');
    const name = Buffer.from('museum-05 exhibit (simulated)');
    s.on('data', (d) => {
      buf = Buffer.concat([buf, d]);
      for (;;) {
        if (stage === 'version') { if (buf.length < 12) return; buf = buf.subarray(12); s.write(Buffer.from([1, 1])); stage = 'sec'; continue; }
        if (stage === 'sec') { if (buf.length < 1) return; buf = buf.subarray(1); s.write(Buffer.alloc(4)); stage = 'init'; continue; }
        if (stage === 'init') {
          if (buf.length < 1) return; buf = buf.subarray(1);
          const h = Buffer.alloc(24); h.writeUInt16BE(FW, 0); h.writeUInt16BE(FH, 2);
          Buffer.from([32, 24, 1, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0]).copy(h, 4); h.writeUInt32BE(name.length, 20);
          s.write(Buffer.concat([h, name])); stage = 'msg'; continue;
        }
        if (buf.length < 1) return;
        const t = buf[0];
        const need = t === 0 ? 20 : t === 2 ? (buf.length >= 4 ? 4 + 4 * buf.readUInt16BE(2) : 1e9) : t === 3 ? 10 : t === 4 ? 8 : t === 5 ? 6 : t === 6 ? (buf.length >= 8 ? 8 + buf.readUInt32BE(4) : 1e9) : 1;
        if (buf.length < need) return;
        const m = buf.subarray(0, need); buf = buf.subarray(need);
        if (t === 5 && m[1] & 1) { const x = m.readUInt16BE(2), y = m.readUInt16BE(4); if (!clicks.some(([a, b]) => Math.abs(a - x) < 4 && Math.abs(b - y) < 4)) clicks.push([x, y]); }
        if (t === 3) {
          const head = Buffer.alloc(16); head[0] = 0; head.writeUInt16BE(1, 2);
          head.writeUInt16BE(FW, 8); head.writeUInt16BE(FH, 10); head.writeInt32BE(0, 12);
          s.write(Buffer.concat([head, exhibit()]));
        }
      }
    });
    s.on('error', () => {});
  });
  srv.listen(VNC, '127.0.0.1');
  return srv;
}

// ── a terminal, just enough of one to read what the seat writes ──────────────
function terminal(cols, rows) {
  const blank = () => ({ ch: ' ', fg: null, bg: null, bold: false, dim: false, rev: false });
  const grid = Array.from({ length: rows }, () => Array.from({ length: cols }, blank));
  let r = 0, c = 0, attr = { fg: null, bg: null, bold: false, dim: false, rev: false };
  const ANSI = { 30: '#000', 31: '#d05050', 32: '#6cbf6c', 33: '#e0b050', 34: '#5080d0', 35: '#b070c0', 36: '#50b0b0', 37: '#ddd', 90: '#808080' };
  const sgr = (ps) => {
    if (!ps.length) ps = [0];
    for (let i = 0; i < ps.length; i++) {
      const p = ps[i];
      if (p === 0) attr = { fg: null, bg: null, bold: false, dim: false, rev: false };
      else if (p === 1) attr.bold = true; else if (p === 2) attr.dim = true; else if (p === 7) attr.rev = true;
      else if (p === 38 && ps[i + 1] === 2) { attr.fg = `rgb(${ps[i + 2]},${ps[i + 3]},${ps[i + 4]})`; i += 4; }
      else if (p === 48 && ps[i + 1] === 2) { attr.bg = `rgb(${ps[i + 2]},${ps[i + 3]},${ps[i + 4]})`; i += 4; }
      else if (ANSI[p]) attr.fg = ANSI[p];
    }
  };
  const clearFrom = (r0, c0) => { for (let y = r0; y < rows; y++) for (let x = y === r0 ? c0 : 0; x < cols; x++) grid[y][x] = blank(); };
  let pend = '';
  return {
    grid,
    feed(s) {
      s = pend + s; pend = '';
      for (let i = 0; i < s.length; i++) {
        const ch = s[i];
        if (ch === '\x1b') {
          const m = /^\x1b\[([?]?)([0-9;]*)([A-Za-z])/.exec(s.slice(i));
          if (!m) { if (s.length - i < 32) { pend = s.slice(i); return; } continue; }
          i += m[0].length - 1;
          const ps = m[2] ? m[2].split(';').map(Number) : [];
          if (m[1]) continue;
          if (m[3] === 'H') { r = (ps[0] || 1) - 1; c = (ps[1] || 1) - 1; }
          else if (m[3] === 'J') { if (ps[0] === 2) clearFrom(0, 0); else clearFrom(r, c); }
          else if (m[3] === 'K') { for (let x = c; x < cols; x++) grid[r][x] = blank(); }
          else if (m[3] === 'm') sgr(ps);
          continue;
        }
        if (ch === '\r') { c = 0; continue; }
        if (ch === '\n') { r = Math.min(rows - 1, r + 1); continue; }
        if (c < cols && r < rows) grid[r][c] = { ch, ...attr };
        c++;
      }
    },
  };
}

// ── the browser ──────────────────────────────────────────────────────────────
let ws, seq = 0, chrome; const pending = new Map();
const send = (method, params = {}) => new Promise((res) => { const i = ++seq; pending.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async (expr) => {
  const r = await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true });
  if (r.result?.exceptionDetails) throw new Error('page: ' + (r.result.exceptionDetails.exception?.description || r.result.exceptionDetails.text).slice(0, 300));
  return r.result?.result?.value;
};
const waitFor = async (expr, ms = 15000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { try { if (await ev(expr)) return true; } catch {} await sleep(150); } throw new Error('timed out waiting for ' + expr); };
const size = (width, height, mobile = false, dpr = 1) => send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: dpr, mobile });
const go = async (url) => { await send('Page.navigate', { url }); await sleep(300); await waitFor('document.readyState==="complete"'); };

// The rings and numbered captions, as in TrendCatcher's guide.
const OVERLAY = `
window.__clear=()=>document.querySelectorAll('.__g').forEach(e=>e.remove());
window.__mark=(items)=>{
  __clear(); const vw=innerWidth, vh=innerHeight, found=[], rings=[], caps=[], host=document.querySelector('dialog[open]')||document.body;
  const over=(a,b)=>Math.max(0,Math.min(a.x+a.w,b.x+b.w)-Math.max(a.x,b.x))*Math.max(0,Math.min(a.y+a.h,b.y+b.h)-Math.max(a.y,b.y));
  items.forEach(([el])=>{
    if(!el){found.push(false);rings.push(null);return;} found.push(true);
    const r=el.getBoundingClientRect();
    const x=Math.max(4,r.left-4), y=Math.max(4,r.top-4), w=Math.min(vw-8-x,r.width+8), h=Math.min(vh-8-y,r.height+8);
    const ring=document.createElement('div'); ring.className='__g';
    ring.style.cssText='position:fixed;z-index:99998;pointer-events:none;border:3px solid #c2185b;border-radius:4px;box-shadow:0 0 0 2px #fff;left:'+x+'px;top:'+y+'px;width:'+w+'px;height:'+h+'px';
    host.appendChild(ring); rings.push({x,y,w,h});
  });
  items.forEach(([el,text],i)=>{
    const g=rings[i]; if(!g)return;
    const cap=document.createElement('div'); cap.className='__g';
    const n=document.createElement('b'); n.textContent=String(i+1);
    n.style.cssText='display:inline-block;background:#fff;color:#c2185b;border-radius:50%;width:20px;height:20px;line-height:20px;text-align:center;margin-right:7px';
    cap.appendChild(n); cap.appendChild(document.createTextNode(text));
    cap.style.cssText='position:fixed;z-index:99999;pointer-events:none;max-width:340px;background:#c2185b;color:#fff;font:600 13px/1.35 system-ui,sans-serif;padding:6px 10px;border-radius:4px;box-shadow:0 2px 8px rgba(0,0,0,.4);left:0;top:0';
    host.appendChild(cap);
    const c=cap.getBoundingClientRect(), cw=c.width, chh=c.height, m=8;
    const spots=[[g.x,g.y+g.h+m],[g.x+g.w-cw,g.y+g.h+m],[g.x,g.y-chh-m],[g.x+g.w-cw,g.y-chh-m],[g.x+g.w+m,g.y],[g.x-cw-m,g.y],
      [g.x+g.w-cw-m,g.y+g.h-chh-m],[g.x+g.w-cw-m,g.y+m],[g.x+m,g.y+g.h-chh-m]];
    let best=null;
    spots.forEach(([sx,sy],k)=>{
      const fits=sx>=4&&sy>=4&&sx+cw<=vw-4&&sy+chh<=vh-4;
      const x=Math.min(Math.max(4,sx),vw-cw-4), y=Math.min(Math.max(4,sy),vh-chh-4), box={x,y,w:cw,h:chh};
      const cost=rings.reduce((a,r,j)=>a+(r?over(box,r)*(j===i?(k>=6?0.02:1):3):0),0)+caps.reduce((a,r)=>a+over(box,r)*10,0)+(fits?0:500)+k;
      if(!best||cost<best.cost)best={x,y,cost};
    });
    cap.style.left=best.x+'px'; cap.style.top=best.y+'px'; caps.push({x:best.x,y:best.y,w:cw,h:chh});
  });
  return found;
};
// A ring around a rectangle of a picture rather than an element: x, y, w, h in
// the picture's own pixels, scaled as the picture is.
window.__union=(...els)=>{const rs=els.map(e=>e.getBoundingClientRect());const l=Math.min(...rs.map(r=>r.left)),t=Math.min(...rs.map(r=>r.top)),
  r=Math.max(...rs.map(r=>r.right)),b=Math.max(...rs.map(r=>r.bottom));const d=document.createElement('div');d.className='__u';
  d.style.cssText='position:fixed;pointer-events:none;left:'+l+'px;top:'+t+'px;width:'+(r-l)+'px;height:'+(b-t)+'px';document.body.appendChild(d);return d;};
window.__box=(x,y,w,h,s)=>{const d=document.createElement('div');d.className='__b';
  d.style.cssText='position:absolute;left:'+x*s+'px;top:'+y*s+'px;width:'+w*s+'px;height:'+h*s+'px';
  document.getElementById('stage').appendChild(d);return d;};`;

let shotN = 0;
async function shot(name, marks = []) {
  await ev(OVERLAY);
  const found = await ev(`__mark([${marks.map(([e, t]) => `[${e},${JSON.stringify(t)}]`).join(',')}])`);
  (found || []).forEach((ok, i) => { if (!ok) say(`   ! nothing to ring for ${name}: ${marks[i][1]}`); });
  await sleep(150);
  const file = `${String(++shotN).padStart(2, '0')}-${name}.png`;
  // Without this, headless Chromium sometimes stops producing frames for a
  // tab and the screenshot waits minutes for one.
  await send('Page.bringToFront');
  await ev('new Promise(r=>{setTimeout(r,3000);requestAnimationFrame(()=>requestAnimationFrame(r))})');
  const r = await send('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path.join(SHOTS, file), Buffer.from(r.result.data, 'base64'));
  await ev(`__clear(); document.querySelectorAll('.__u').forEach(e=>e.remove())`);
  say(`   ${file}  ${(fs.statSync(path.join(SHOTS, file)).size / 1024).toFixed(0)} KB`);
}

// A frame from --frame, captured at twice its size with nearest-neighbour pixels
// (the letters are bitmaps), and rings placed in the frame's own coordinates.
const SCALE = 2;
async function frameShot(name, args, boxes, env) {
  const ppm = path.join(BOX, name + '.ppm');
  await run(['--frame', ppm, '--demo', ...args], { env, cwd: path.join(BOX, 'exhibit') });
  const { w, h, png } = ppmToPng(fs.readFileSync(ppm));
  const html = path.join(BOX, name + '.html');
  fs.writeFileSync(html, `<!doctype html><meta charset=utf-8><style>html,body{margin:0;background:#000}#stage{position:relative;width:${w}px;height:${h}px}img{width:100%;height:100%;image-rendering:pixelated;display:block}</style><div id=stage><img src="data:image/png;base64,${png.toString('base64')}"></div>`);
  await size(w, h, false, SCALE);
  await go('file://' + html);
  await shot(name, boxes.map(([x, y, bw, bh, t]) => [`__box(${x},${y},${bw},${bh},1)`, t]));
}

// The seat, in a real pseudo-terminal, read back through `terminal()` and drawn
// on a canvas cell by cell: a half block is two rectangles, so nothing depends
// on a font drawing U+2580 to the exact height of its line.
const TC = 150, TR = 46, CW = 8, CH = 16;
async function termShot(name, term, boxes) {
  const cells = term.grid.map((row) => row.map((g) => [g.ch, g.fg, g.bg, g.bold ? 1 : 0, g.dim ? 1 : 0, g.rev ? 1 : 0]));
  const html = path.join(BOX, name + '.html');
  fs.writeFileSync(html, `<!doctype html><meta charset=utf-8><style>html,body{margin:0;background:#161616}body{padding:24px}#stage{position:relative}canvas{display:block}</style><div id=stage><canvas id=c width=${TC * CW} height=${TR * CH}></canvas></div><script>
const cells=${JSON.stringify(cells)}, CW=${CW}, CH=${CH}, FG='#d8d8d8', BG='#161616';
const x=document.getElementById('c').getContext('2d'); x.textBaseline='top';
cells.forEach((row,r)=>row.forEach(([ch,fg,bg,b,d,v],c)=>{
  let f=fg||FG, k=bg||BG; if(v){const t=f;f=k;k=t;}
  x.globalAlpha=1; x.fillStyle=k; x.fillRect(c*CW,r*CH,CW,CH);
  if(ch==='\\u2580'){x.fillStyle=f;x.fillRect(c*CW,r*CH,CW,CH/2);return;}
  if(ch===' ')return;
  x.globalAlpha=d?0.55:1; x.fillStyle=f; x.font=(b?'bold ':'')+'13px monospace'; x.fillText(ch,c*CW,r*CH+1);
}));</script>`);
  await size(TC * CW + 48, TR * CH + 48);
  await go('file://' + html);
  await shot(name, boxes.map(([c, r, w, h, t]) => [`__box(${c * CW},${r * CH},${w * CW},${h * CH},1)`, t]));
}

// ── run ──────────────────────────────────────────────────────────────────────
try {
  if (!fs.existsSync(BIN)) throw new Error(`no ${BIN} -- cargo build --release first`);
  fs.rmSync(BOX, { recursive: true, force: true });
  fs.mkdirSync(path.join(BOX, 'home'), { recursive: true });
  // The local side of the Exchange frame: a made-up exhibit folder.
  fs.mkdirSync(path.join(BOX, 'exhibit/posters'), { recursive: true });
  fs.writeFileSync(path.join(BOX, 'exhibit/caption.txt'), 'The orrery turns once a minute.\n');
  fs.writeFileSync(path.join(BOX, 'exhibit/scene-show.json'), '{"scene":"show"}\n');
  fs.rmSync(SHOTS, { recursive: true, force: true }); fs.mkdirSync(SHOTS, { recursive: true });

  // ═══ the wall ══════════════════════════════════════════════════════════════
  serve(['--demo', '--listen', `127.0.0.1:${GALLERY}`, '--quiet']);
  serve(['--demo', '--operator', TOKEN, '--listen', `127.0.0.1:${OPERATOR}`, '--quiet']);
  for (let i = 0; i < 40; i++) { await sleep(150); try { if ((await fetch(`http://127.0.0.1:${GALLERY}/healthz`)).ok && (await fetch(`http://127.0.0.1:${OPERATOR}/healthz`)).ok) break; } catch {} }

  chrome = spawn('chromium', ['--headless=new', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', `--remote-debugging-port=${CDP}`, `--user-data-dir=${path.join(BOX, 'chrome')}`, 'about:blank'], { stdio: 'ignore' });
  children.push(chrome);
  let targets;
  for (let i = 0; i < 60; i++) { await sleep(250); try { targets = await (await fetch(`http://127.0.0.1:${CDP}/json`)).json(); if (targets.some((t) => t.type === 'page')) break; } catch {} }
  ws = new WebSocket(targets.find((t) => t.type === 'page').webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));
  ws.onmessage = (m) => { const j = JSON.parse(m.data); if (j.id && pending.has(j.id)) { pending.get(j.id)(j); pending.delete(j.id); } };
  await send('Runtime.enable'); await send('Page.enable');
  // The page follows the system theme; pin the light one so the pictures agree.
  await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'light' }] });

  const tileOf = (id) => `[...document.querySelectorAll('.tile')].find(t=>t.querySelector('.t-id').textContent===${JSON.stringify(id)})`;
  const btn = (label) => `[...document.querySelectorAll('#verbs button')].find(b=>b.textContent.includes(${JSON.stringify(label)}))`;

  say('the wall, gallery posture');
  await size(1280, 760);
  await go(`http://127.0.0.1:${GALLERY}/`);
  await waitFor(`document.querySelectorAll('.tile').length===8`);
  await shot('wall-gallery', [
    ['__union(document.querySelector(".name"),document.getElementById("bus"))', 'The fleet, how many are up, and whether the bus is live: all from copal fleet state.'],
    ['document.getElementById("filters")', 'Tags from the document, and the warden.'],
    [tileOf('museum-07'), 'Declared, never announced: no temperature is drawn, because none was reported.'],
    ['document.getElementById("notes")', 'A stranger on the LAN, seen and never contacted; and the node that needs a human.'],
    ['document.getElementById("verbs")', 'Gallery posture: every write verb is greyed, and the write route does not exist.'],
  ]);

  say('the wall on a phone');
  await size(390, 844, true);
  await go(`http://127.0.0.1:${GALLERY}/`);
  await waitFor(`document.querySelectorAll('.tile').length===8`);
  await ev(`document.querySelector('.chip:nth-child(2)') && [...document.querySelectorAll('.chip')].find(c=>c.textContent==='north').click()`);
  await sleep(300);
  await shot('wall-phone', [
    [`[...document.querySelectorAll('.chip')].find(c=>c.textContent==='north')`, 'One tag chosen: the wall shows only those nodes.'],
    [tileOf('museum-06'), 'The warden, which serves this page.'],
  ]);

  say('the wall, operator posture');
  await size(1280, 760);
  await go(`http://127.0.0.1:${OPERATOR}/`);
  await waitFor(`document.querySelectorAll('.tile').length===8`);
  for (const id of ['museum-01', 'museum-03', 'museum-05']) await ev(`${tileOf(id)}.click()`);
  await sleep(300);
  await shot('wall-operator', [
    [tileOf('museum-03'), 'Click to select; the accent bar marks each chosen tile.'],
    ['document.getElementById("sel")', 'Three selected.'],
    [btn('Scene'), 'Scene, Run, Power and Snapshot act on all of them, one node at a time.'],
    [btn('Control'), 'Control, Observe and Exchange want exactly one node, so they are greyed.'],
  ]);

  say('a verb from the wall');
  await ev(`act(VERBS.find(v=>v.id==='scene'))`);
  await waitFor(`document.getElementById('dlg').open`);
  await sleep(200);
  await shot('wall-scene', [
    ['document.querySelector("#dlgbody p")', 'What it will act on, named before anything runs.'],
    ['document.querySelector("#dlgbody select")', 'The scenes are the read model\'s own list, not free text.'],
  ]);
  // "Do it": what the dialog's close handler does, called directly so the
  // picture does not depend on when the close event lands.
  await ev(`(()=>{const d=document.getElementById('dlg'); d.onclose=null; d.close('cancel'); return post('scene','show');})()`);
  await waitFor(`document.getElementById('dlg').open && document.querySelector('#dlgbody pre')`);
  await sleep(200);
  await shot('wall-result', [
    ['document.querySelector("#dlgbody pre")', 'The page posts without the X-Copal-Token header, so the console refuses it (see "One finding").'],
  ]);
  await ev(`document.getElementById('dlg').close()`);

  say('the web seat panel');
  await size(1280, 900);
  await ev(`sel.clear(); openSeat('museum-05')`);
  await sleep(300);
  await shot('wall-seat', [
    ['document.getElementById("seatverbs")', 'Observe, Control, Exchange for one node.'],
    ['document.getElementById("stage")', 'No screen is proxied: the page says what would be needed instead.'],
    ['document.getElementById("facts")', 'Facts from the read model; "not collected" and "not reported" are kept apart.'],
  ]);

  // ═══ the seat ══════════════════════════════════════════════════════════════
  say('the seat, against a simulated node');
  const doc = JSON.parse(await (await fetch(`http://127.0.0.1:${GALLERY}/api/state`)).text());
  for (const k of Object.keys(doc)) if (k.startsWith('_')) delete doc[k];
  doc.nodes.find((n) => n.id === 'museum-05').address = '127.0.0.1';
  const stateFile = path.join(BOX, 'state.json');
  fs.writeFileSync(stateFile, JSON.stringify(doc));
  const vnc = vncServer();
  const seatCmd = [BIN, '--seat', 'museum-05', '--vnc-port', String(VNC), '--half-block', '--fps', '4',
    '--fleet-cmd', `${process.execPath} ${path.join(ROOT, 'tools/feature-guide.mjs')} --fleet-state ${stateFile}`]
    .map((a) => `'${a.replace(/'/g, `'\\''`)}'`).join(' ');
  const seat = spawn('script', ['-qfec', `stty rows ${TR} cols ${TC}; exec ${seatCmd}`, '/dev/null'], { env: { ...process.env, HOME: path.join(BOX, 'home'), TERM: 'xterm-256color' }, stdio: ['pipe', 'pipe', 'pipe'] });
  children.push(seat);
  const term = terminal(TC, TR);
  const dec = new TextDecoder();
  let seatErr = '';
  seat.stdout.on('data', (d) => term.feed(dec.decode(d, { stream: true })));
  seat.stderr.on('data', (d) => (seatErr += d));
  await sleep(3000);
  if (!term.grid[0].some((g) => g.ch !== ' ')) throw new Error('the seat drew nothing: ' + seatErr);
  await termShot('seat-observe', term, [
    [0, 0, 64, 2, 'The node, its status and scene from copal fleet state, the desktop\'s own name; and the four faces, one key each.'],
    [30, 9, 90, 28, 'The node\'s screen in half-blocks: two pixels a cell, 24-bit colour.'],
    [0, TR - 1, 80, 1, 'The keys, and which renderer it chose.'],
  ]);
  seat.stdin.write('c');
  await sleep(600);
  // A click, as the terminal reports it (SGR mouse), on the third planet.
  seat.stdin.write('\x1b[<0;101;19M');
  await sleep(150);
  seat.stdin.write('\x1b[<0;101;19m');
  await sleep(2000);
  // Keep the picture, then let go (Ctrl-]) and quit (q) before drawing it.
  const held = { grid: term.grid.map((row) => row.map((g) => ({ ...g }))) };
  seat.stdin.write('\x1d');
  await sleep(300);
  seat.stdin.write('q');
  await sleep(500);
  vnc.close();
  await termShot('seat-control', held, [
    [0, 1, 50, 1, 'Control is on.'],
    [94, 14, 14, 9, 'A click in the terminal, delivered to the node as a pointer event: the (simulated) node drew a mark where it landed.'],
    [0, TR - 1, 40, 1, 'Ctrl-] is the one way out, and it always works.'],
  ]);

  // ═══ the museum interface, by --frame ══════════════════════════════════════
  say('the museum interface');
  await frameShot('lab-gallery', [], [
    [0, 0, 960, 32, 'Lab and Media; the fleet; live, declared, up, on the bus.'],
    [0, 36, 136, 220, 'The rail: scenes, tags, and strangers, which are never tiles.'],
    [148, 130, 476, 80, 'A warden, a missing node, and one whose agent went silent.'],
    [0, 546, 960, 54, 'Gallery posture: no verb bar at all.'],
  ]);
  await frameShot('lab-many', ['--operator', 'x', '--frame-select', 'museum-01,museum-03,museum-05'], [
    [472, 42, 152, 78, 'A selected tile wears a heavier frame.'],
    [8, 550, 410, 24, 'Only the verbs that mean something on many: Send, Message, Run, Scene, Snapshot, Power.'],
    [855, 580, 105, 20, '"1 not built": Observe, refused rather than drawn.'],
  ]);
  await frameShot('lab-one', ['--operator', 'x', '--frame-select', 'museum-06'], [
    [148, 130, 152, 78, 'One node: the warden.'],
    [8, 550, 260, 24, 'Control, Terminal and Exchange appear, because they mean one machine.'],
    [0, 92, 136, 14, 'Its scene, lit in the rail.'],
  ]);
  await frameShot('lab-message', ['--operator', 'x', '--frame-select', 'museum-01,museum-02'], [
    [0, 548, 960, 28, 'Message asks for the banner in the bar itself: no dialog, no window inside the window.'],
    [0, 116, 136, 66, 'The tag "wall" is lit: the selection is exactly its two nodes.'],
  ], { ORRERY_FRAME_VERB: 'Message' });
  await frameShot('lab-dark', ['--operator', 'x', '--dark', '--frame-select', 'museum-03'], [
    [472, 42, 152, 78, 'The night palette (--dark): the same frame, the same rules.'],
  ]);
  await frameShot('lab-exchange', ['--frame-view', 'files', '--frame-select', 'museum-01'], [
    [0, 26, 440, 20, 'This machine (a made-up exhibit folder here).'],
    [520, 26, 440, 20, 'The node, over SFTP (a fixture in this frame).'],
    [440, 60, 80, 54, 'Copy either way.'],
    [0, 572, 520, 24, 'Keyboard first: Tab, Return, Escape.'],
  ]);
  await frameShot('media', ['--frame-view', 'media'], [
    [0, 40, 960, 180, 'The card ledger: each card, its token, when it was built, and whether its node is up.'],
    [0, 512, 480, 28, 'Which board the next card is for.'],
    [0, 550, 960, 48, 'Write card runs copal-prep.sh on a real terminal; its two typed ERASE confirmations stay with you.'],
  ]);
  say('done');
} catch (e) {
  console.error('feature-guide:', e.message);
  process.exitCode = 1;
} finally {
  stop();
  await sleep(800); // let Chromium let go of its profile folder
  if (!process.argv.includes('--keep')) fs.rmSync(BOX, { recursive: true, force: true });
}
