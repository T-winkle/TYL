// Rasterize the shared vector mark for Windows resources and the system tray.
// Usage: node scripts/render-logo.mjs [path-to-sharp-module]
import { createRequire } from "node:module";
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
const require = createRequire(import.meta.url);
const sharp = require(process.argv[2] || "sharp");
const source = await readFile(
  new URL("../src/shared/logo.svg", import.meta.url),
);
const dir = new URL("../../src-tauri/icons/", import.meta.url);
for (const [file, size] of [
  ["32x32.png", 32],
  ["128x128.png", 128],
  ["icon.png", 512],
]) {
  await sharp(source, { density: 768 })
    .resize(size, size)
    .png()
    .toFile(fileURLToPath(new URL(file, dir)));
}
const sizes = [16, 24, 32, 48, 64, 128, 256];
const images = await Promise.all(
  sizes.map((size) =>
    sharp(source, { density: 768 }).resize(size, size).png().toBuffer(),
  ),
);
const header = Buffer.alloc(6 + sizes.length * 16);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(sizes.length, 4);
let offset = header.length;
images.forEach((data, index) => {
  const entry = 6 + index * 16;
  header[entry] = sizes[index] === 256 ? 0 : sizes[index];
  header[entry + 1] = header[entry];
  header.writeUInt16LE(1, entry + 4);
  header.writeUInt16LE(32, entry + 6);
  header.writeUInt32LE(data.length, entry + 8);
  header.writeUInt32LE(offset, entry + 12);
  offset += data.length;
});
await writeFile(new URL("icon.ico", dir), Buffer.concat([header, ...images]));
console.log("Rendered logo: PNG 32/128/512 and multi-resolution Windows ICO.");
