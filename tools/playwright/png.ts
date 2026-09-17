// Minimal PNG reader for screenshot assertions. Handles the 8-bit RGB/RGBA
// files the renderer emits; Node has no built-in image decoder and the test
// suite intentionally has no image dependencies.
import { inflateSync } from "node:zlib";

export interface DecodedPng {
  width: number;
  height: number;
  channels: number;
  data: Buffer;
}

export function decodePng(png: Buffer): DecodedPng {
  let offset = 8;
  let width = 0;
  let height = 0;
  let channels = 4;
  const idat: Buffer[] = [];
  while (offset + 8 <= png.length) {
    const length = png.readUInt32BE(offset);
    const type = png.toString("ascii", offset + 4, offset + 8);
    const data = png.subarray(offset + 8, offset + 8 + length);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      const bitDepth = data[8];
      const colorType = data[9];
      if (bitDepth !== 8) throw new Error(`unsupported bit depth ${bitDepth}`);
      if (colorType === 6) channels = 4;
      else if (colorType === 2) channels = 3;
      else throw new Error(`unsupported color type ${colorType}`);
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
    offset += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const out = Buffer.alloc(height * stride);
  let position = 0;
  for (let y = 0; y < height; y++) {
    const filter = raw[position++];
    const row = raw.subarray(position, position + stride);
    position += stride;
    const previous = y === 0 ? null : out.subarray((y - 1) * stride, y * stride);
    const target = out.subarray(y * stride, (y + 1) * stride);
    for (let i = 0; i < stride; i++) {
      const left = i >= channels ? target[i - channels] : 0;
      const above = previous ? previous[i] : 0;
      const upperLeft = previous && i >= channels ? previous[i - channels] : 0;
      let value = row[i];
      switch (filter) {
        case 0:
          break;
        case 1:
          value = (value + left) & 0xff;
          break;
        case 2:
          value = (value + above) & 0xff;
          break;
        case 3:
          value = (value + ((left + above) >> 1)) & 0xff;
          break;
        case 4: {
          const estimate = left + above - upperLeft;
          const distanceLeft = Math.abs(estimate - left);
          const distanceAbove = Math.abs(estimate - above);
          const distanceUpperLeft = Math.abs(estimate - upperLeft);
          const prediction =
            distanceLeft <= distanceAbove && distanceLeft <= distanceUpperLeft
              ? left
              : distanceAbove <= distanceUpperLeft
                ? above
                : upperLeft;
          value = (value + prediction) & 0xff;
          break;
        }
        default:
          throw new Error(`unsupported filter ${filter}`);
      }
      target[i] = value;
    }
  }
  return { width, height, channels, data: out };
}

export function pixel(image: DecodedPng, x: number, y: number): [number, number, number] {
  const index = (y * image.width + x) * image.channels;
  return [image.data[index], image.data[index + 1], image.data[index + 2]];
}
