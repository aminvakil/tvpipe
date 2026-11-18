/**
 * SLRW records (docs/slrw.md, docs/machine.md): what the SDK writes to the
 * binary's stdin.
 *
 * `u8 kind | u64le pts_us | u32le len | payload`, after the magic `SLRW`.
 */

export const MAGIC: Uint8Array = new TextEncoder().encode("SLRW");

export const Kind = {
  end: 0,
  video: 1,
  audio: 2,
  h264: 3,
  h264Config: 4,
  control: 5,
  command: 6,
} as const;

export type KindCode = (typeof Kind)[keyof typeof Kind];

export const H264_KEYFRAME = 0x01;
export const H264_ANNEX_B = 0x02;
export const CONFIG_AVCC = 0;
export const CONFIG_ANNEX_B = 1;
export const CONTROL_KEYFRAME_REQUESTS = 0x01;
export const HEADER_LEN = 13;

/** Microseconds as the record's u64. */
function toPts(ptsUs: number | bigint): bigint {
  if (typeof ptsUs === "bigint") {
    if (ptsUs < 0n) throw new RangeError("pts must not be negative");
    return ptsUs;
  }
  if (!Number.isFinite(ptsUs) || ptsUs < 0) throw new RangeError(`invalid pts ${ptsUs}`);
  return BigInt(Math.round(ptsUs));
}

/**
 * One record in a single new buffer: the header, an optional prefix byte
 * (the flags or format of encoded kinds) and the payload, copied (callers
 * may reuse their buffers right away).
 */
export function record(
  kind: KindCode,
  ptsUs: number | bigint,
  payload: Uint8Array,
  prefix?: number,
): Buffer {
  const extra = prefix === undefined ? 0 : 1;
  const len = payload.byteLength + extra;
  const buf = Buffer.allocUnsafe(HEADER_LEN + len);
  buf.writeUInt8(kind, 0);
  buf.writeBigUInt64LE(toPts(ptsUs), 1);
  buf.writeUInt32LE(len, 9);
  if (prefix !== undefined) buf.writeUInt8(prefix, HEADER_LEN);
  buf.set(payload, HEADER_LEN + extra);
  return buf;
}

/** A `command` record (machine interface). */
export function command(cmd: Record<string, unknown>): Buffer {
  return record(Kind.command, 0, new TextEncoder().encode(JSON.stringify(cmd)));
}

/** A `control` record. */
export function control(flags: number): Buffer {
  const payload = new Uint8Array(4);
  new DataView(payload.buffer).setUint32(0, flags >>> 0, true);
  return record(Kind.control, 0, payload);
}

/** Bytes of an `ArrayBufferView` or `ArrayBuffer`, without copying. */
export function bytes(data: ArrayBufferView | ArrayBuffer): Uint8Array {
  if (data instanceof Uint8Array) return data;
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  return new Uint8Array(data);
}

/** Interleaved float samples (-1..1) as s16le bytes. */
export function floatToS16(samples: Float32Array | Float64Array): Uint8Array {
  const out = new Uint8Array(samples.length * 2);
  const view = new DataView(out.buffer);
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i] ?? 0));
    view.setInt16(i * 2, Math.round(s < 0 ? s * 0x8000 : s * 0x7fff), true);
  }
  return out;
}

/** Whether an access unit starts with an Annex B start code. */
export function isAnnexB(au: Uint8Array): boolean {
  return (
    au.length >= 4 &&
    au[0] === 0 &&
    au[1] === 0 &&
    (au[2] === 1 || (au[2] === 0 && au[3] === 1))
  );
}
