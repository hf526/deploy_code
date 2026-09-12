import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = resolve(__dirname, "../src-tauri/icons");

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, "ascii");
  const crcBuf = Buffer.alloc(4);
  crcBuf.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])), 0);
  return Buffer.concat([len, typeBuf, data, crcBuf]);
}

function encodePng(width, height, rgba) {
  const signature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0;
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }
  return Buffer.concat([
    signature,
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", deflateSync(raw, { level: 9 })),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

function encodeIcoBmp(width, height, rgba) {
  const bitmapHeader = Buffer.alloc(40);
  bitmapHeader.writeUInt32LE(40, 0);
  bitmapHeader.writeInt32LE(width, 4);
  bitmapHeader.writeInt32LE(height * 2, 8);
  bitmapHeader.writeUInt16LE(1, 12);
  bitmapHeader.writeUInt16LE(32, 14);
  bitmapHeader.writeUInt32LE(0, 16);
  bitmapHeader.writeUInt32LE(width * height * 4, 20);

  const xor = Buffer.alloc(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const src = (y * width + x) * 4;
      const dst = ((height - 1 - y) * width + x) * 4;
      xor[dst] = rgba[src + 2];
      xor[dst + 1] = rgba[src + 1];
      xor[dst + 2] = rgba[src];
      xor[dst + 3] = rgba[src + 3];
    }
  }

  const maskStride = Math.ceil(width / 32) * 4;
  const and = Buffer.alloc(maskStride * height, 0);

  const image = Buffer.concat([bitmapHeader, xor, and]);

  const dir = Buffer.alloc(6);
  dir.writeUInt16LE(0, 0);
  dir.writeUInt16LE(1, 2);
  dir.writeUInt16LE(1, 4);

  const entry = Buffer.alloc(16);
  entry[0] = width >= 256 ? 0 : width;
  entry[1] = height >= 256 ? 0 : height;
  entry[2] = 0;
  entry[3] = 0;
  entry.writeUInt16LE(1, 4);
  entry.writeUInt16LE(32, 6);
  entry.writeUInt32LE(image.length, 8);
  entry.writeUInt32LE(6 + 16, 12);

  return Buffer.concat([dir, entry, image]);
}

function smoothstep(edge0, edge1, x) {
  const t = Math.min(1, Math.max(0, (x - edge0) / (edge1 - edge0)));
  return t * t * (3 - 2 * t);
}

function sdRoundRect(px, py, cx, cy, halfW, halfH, r) {
  const qx = Math.abs(px - cx) - (halfW - r);
  const qy = Math.abs(py - cy) - (halfH - r);
  const ax = Math.max(qx, 0);
  const ay = Math.max(qy, 0);
  return Math.sqrt(ax * ax + ay * ay) + Math.min(Math.max(qx, qy), 0) - r;
}

function sdSegment(px, py, ax, ay, bx, by) {
  const vx = bx - ax;
  const vy = by - ay;
  const wx = px - ax;
  const wy = py - ay;
  const len2 = vx * vx + vy * vy || 1;
  const t = Math.min(1, Math.max(0, (wx * vx + wy * vy) / len2));
  const dx = wx - vx * t;
  const dy = wy - vy * t;
  return Math.sqrt(dx * dx + dy * dy);
}

function bezierPoints(x0, y0, x1, y1, x2, y2, steps = 240) {
  const pts = [];
  for (let i = 0; i <= steps; i++) {
    const t = i / steps;
    const mt = 1 - t;
    pts.push([
      mt * mt * x0 + 2 * mt * t * x1 + t * t * x2,
      mt * mt * y0 + 2 * mt * t * y1 + t * t * y2,
    ]);
  }
  return pts;
}

function sdPolyline(px, py, pts) {
  let best = Infinity;
  for (let i = 0; i < pts.length - 1; i++) {
    const d = sdSegment(px, py, pts[i][0], pts[i][1], pts[i + 1][0], pts[i + 1][1]);
    if (d < best) best = d;
  }
  return best;
}

function render(size) {
  const rgba = Buffer.alloc(size * size * 4);
  const trunk = bezierPoints(0.36 * size, 0.76 * size, 0.36 * size, 0.55 * size, 0.36 * size, 0.24 * size, 80);
  const branch = bezierPoints(0.36 * size, 0.6 * size, 0.62 * size, 0.6 * size, 0.66 * size, 0.44 * size, 240);
  const lineW = 0.052 * size;
  const nodeR = 0.088 * size;
  const nodes = [
    [0.36, 0.24],
    [0.36, 0.76],
    [0.66, 0.44],
  ];

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const px = x + 0.5;
      const py = y + 0.5;

      let r = 0;
      let g = 0;
      let b = 0;
      let a = 0;

      const bgDist = sdRoundRect(px, py, size / 2, size / 2, size / 2 - 0.5, size / 2 - 0.5, 0.21 * size);
      const bgAlpha = 1 - smoothstep(-1.2, 1.2, bgDist);
      if (bgAlpha > 0) {
        const t = Math.min(1, Math.max(0, (x + y) / (2 * size)));
        r = Math.round(99 + (139 - 99) * t);
        g = Math.round(102 + (92 - 102) * t);
        b = Math.round(241 + (246 - 241) * t);
        a = 255 * bgAlpha;
      }

      const glyphDist = Math.min(
        sdPolyline(px, py, trunk),
        sdPolyline(px, py, branch),
        ...nodes.map(([nx, ny]) => Math.sqrt((px - nx * size) ** 2 + (py - ny * size) ** 2) - nodeR)
      );
      const glyphAlpha = 1 - smoothstep(-1.1, 1.1, glyphDist - lineW);
      if (glyphAlpha > 0) {
        const fgA = glyphAlpha * bgAlpha;
        const bgA = a / 255;
        const outA = fgA + bgA * (1 - fgA);
        if (outA > 0) {
          r = Math.round((255 * fgA + r * bgA * (1 - fgA)) / outA);
          g = Math.round((255 * fgA + g * bgA * (1 - fgA)) / outA);
          b = Math.round((255 * fgA + b * bgA * (1 - fgA)) / outA);
        }
        a = 255 * outA;
      }

      const idx = (y * size + x) * 4;
      rgba[idx] = r;
      rgba[idx + 1] = g;
      rgba[idx + 2] = b;
      rgba[idx + 3] = Math.min(255, Math.round(a));
    }
  }
  return rgba;
}

mkdirSync(OUT_DIR, { recursive: true });

const pngTargets = [
  ["32x32.png", 32],
  ["128x128.png", 128],
  ["128x128@2x.png", 256],
  ["icon.png", 512],
  ["Square150x150Logo.png", 150],
  ["Square44x44Logo.png", 44],
];
for (const [name, size] of pngTargets) {
  writeFileSync(resolve(OUT_DIR, name), encodePng(size, size, render(size)));
  console.log("generated", name);
}

const icoSize = 256;
writeFileSync(resolve(OUT_DIR, "icon.ico"), encodeIcoBmp(icoSize, icoSize, render(icoSize)));
console.log("generated icon.ico");
