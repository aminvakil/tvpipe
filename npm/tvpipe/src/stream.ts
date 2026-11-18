/**
 * `stream()`: frames from code (raw pixels, PCM, a canvas, encoded H.264)
 * through SLRW on tvpipe's stdin.
 */

import type { EventEmitter } from "node:events";

import { canvasPixels, type CanvasLike, type Context2DLike } from "./canvas.ts";
import { TvpipeError } from "./errors.ts";
import type { TvpipeProcess } from "./process.ts";
import { Session, type EndInfo, type SessionOptions } from "./session.ts";
import * as slrw from "./slrw.ts";

export type PixelFormat = "rgba" | "bgra" | "rgb0" | "bgr0" | "rgb24" | "bgr24" | "nv12" | "yuv420p";

/** Raw frames, encoded by tvpipe. */
export interface RawVideoSpec {
  width: number;
  height: number;
  /** Frames per second (also the pace of {@link StreamSession.drawLoop}). */
  fps: number;
  /** Pixel layout of `writeVideo` frames (default `rgba`; canvases are `rgba`). */
  format?: PixelFormat;
}

/** H.264 encoded by the caller (WebCodecs, a hardware encoder). */
export interface EncodedVideoSpec {
  width: number;
  height: number;
  fps: number;
  codec: "h264";
}

export type VideoSpec = RawVideoSpec | EncodedVideoSpec;

/** Interleaved signed 16-bit PCM (float input is converted). */
export interface AudioSpec {
  sampleRate: number;
  channels: number;
}

/** A WebCodecs `EncodedVideoChunk`, duck-typed. */
export interface EncodedVideoChunkLike {
  readonly type: "key" | "delta";
  /** Microseconds. */
  readonly timestamp: number;
  readonly byteLength: number;
  copyTo(destination: Uint8Array | ArrayBuffer): void;
}

/** WebCodecs `EncodedVideoChunkMetadata`, duck-typed. */
export interface EncodedVideoChunkMetadataLike {
  decoderConfig?: { description?: ArrayBufferView | ArrayBuffer | undefined } | undefined;
}

export type Pts = number | bigint;

const BYTES_PER_PIXEL: Record<PixelFormat, number> = {
  rgba: 4,
  bgra: 4,
  rgb0: 4,
  bgr0: 4,
  rgb24: 3,
  bgr24: 3,
  nv12: 1.5,
  yuv420p: 1.5,
};

export interface StreamSessionOptions extends SessionOptions {
  video: VideoSpec;
  audio?: AudioSpec | undefined;
  /**
   * Raw video: frames are dropped (`writeVideo` returns `false`) while
   * this many frames' worth of bytes wait in the pipe (default 2).
   */
  maxQueuedFrames?: number | undefined;
  /** Playout delay (ms), to wait for the tail in `end()`. */
  latencyMs?: number | undefined;
}

/** A session fed by the program: see {@link stream}. */
export class StreamSession extends Session {
  readonly video: VideoSpec;
  readonly audio: AudioSpec | undefined;
  /** Raw frames skipped because the pipe was backed up. */
  readonly dropped = { video: 0 };
  #t0 = performance.now();
  #lastPts = { video: -1n, audio: -1n };
  #frameBytes: number;
  #maxQueued: number;
  #config: Uint8Array | undefined;
  #latencyMs: number;
  #loops = new Set<() => void>();
  #keyframePending = false;

  constructor(proc: TvpipeProcess, options: StreamSessionOptions) {
    super(proc, options);
    this.video = options.video;
    this.audio = options.audio;
    this.#maxQueued = options.maxQueuedFrames ?? 2;
    this.#latencyMs = options.latencyMs ?? 300;
    const format = "codec" in this.video ? undefined : (this.video.format ?? "rgba");
    this.#frameBytes = format ? Math.ceil(this.video.width * this.video.height * BYTES_PER_PIXEL[format]) : 0;
    if ("codec" in this.video) {
      // Keyframe requests become `keyframe` events.
      proc.write(slrw.control(slrw.CONTROL_KEYFRAME_REQUESTS));
      // The first request comes right when the input opens, often before
      // the caller had a chance to listen: keep it for the first listener.
      this.on("keyframe", () => {
        if (this.listenerCount("keyframe") === 1) this.#keyframePending = true;
      });
      (this as EventEmitter).on("newListener", (event: string | symbol) => {
        if (event === "keyframe" && this.#keyframePending) {
          this.#keyframePending = false;
          process.nextTick(() => this.emit("keyframe", { type: "keyframe" }));
        }
      });
    }
    void this.done.catch(() => {}).finally(() => {
      for (const stop of this.#loops) stop();
    });
  }

  /** Microseconds since the stream started. */
  now(): number {
    return Math.round((performance.now() - this.#t0) * 1000);
  }

  #pts(track: "video" | "audio", pts: Pts | undefined): bigint {
    let p = pts === undefined ? BigInt(this.now()) : typeof pts === "bigint" ? pts : BigInt(Math.round(pts));
    if (p < 0n) throw new TvpipeError("invalid-argument", "pts must not be negative");
    // Defaulted timestamps must increase; given ones are the caller's.
    if (pts === undefined && p <= this.#lastPts[track]) p = this.#lastPts[track] + 1n;
    this.#lastPts[track] = p;
    return p;
  }

  #raw(): RawVideoSpec {
    if ("codec" in this.video) {
      throw new TvpipeError("invalid-argument", "this stream takes H.264 (writeH264/writeChunk), not raw frames");
    }
    return this.video;
  }

  #encoded(): void {
    if (!("codec" in this.video)) {
      throw new TvpipeError("invalid-argument", "this stream takes raw frames; use video: { codec: \"h264\" } for H.264");
    }
  }

  /**
   * One raw frame in the stream's size and pixel format. Returns `false`
   * when the frame was dropped because tvpipe is behind (live sources
   * keep going; see `maxQueuedFrames`) or the stream is over.
   */
  writeVideo(frame: ArrayBufferView | ArrayBuffer, ptsUs?: Pts): boolean {
    this.#raw();
    const data = slrw.bytes(frame);
    if (data.byteLength !== this.#frameBytes) {
      throw new TvpipeError(
        "invalid-argument",
        `a ${this.video.width}x${this.video.height} frame has ${this.#frameBytes} bytes, got ${data.byteLength}`,
      );
    }
    if (!this.proc.inputOpen) return false;
    if (this.proc.queuedBytes >= this.#frameBytes * this.#maxQueued) {
      this.dropped.video++;
      return false;
    }
    this.proc.write(slrw.record(slrw.Kind.video, this.#pts("video", ptsUs), data));
    return true;
  }

  /**
   * Interleaved PCM: `Int16Array` or s16le bytes, or float samples
   * (-1..1). Never dropped. Returns `false` once the pipe is backed up
   * (the data is queued anyway) or the stream is over.
   */
  writeAudio(pcm: Int16Array | Float32Array | Float64Array | Uint8Array | ArrayBuffer, ptsUs?: Pts): boolean {
    if (!this.audio) throw new TvpipeError("invalid-argument", "the stream has no audio (pass audio: { sampleRate, channels })");
    const data = pcm instanceof Float32Array || pcm instanceof Float64Array ? slrw.floatToS16(pcm) : slrw.bytes(pcm);
    if (data.byteLength % (2 * this.audio.channels) !== 0) {
      throw new TvpipeError("invalid-argument", `PCM must be whole frames of ${this.audio.channels} s16 samples`);
    }
    if (data.byteLength === 0) return true;
    return this.proc.write(slrw.record(slrw.Kind.audio, this.#pts("audio", ptsUs), data));
  }

  /** The canvas's current picture (via `getImageData`; the stream's format must be `rgba`). */
  writeCanvas(canvas: CanvasLike, ptsUs?: Pts): boolean {
    const raw = this.#raw();
    if ((raw.format ?? "rgba") !== "rgba") {
      throw new TvpipeError("invalid-argument", "canvases give rgba pixels; create the stream with format \"rgba\"");
    }
    if (canvas.width !== raw.width || canvas.height !== raw.height) {
      throw new TvpipeError(
        "invalid-argument",
        `the canvas is ${canvas.width}x${canvas.height}, the stream ${raw.width}x${raw.height}`,
      );
    }
    return this.writeVideo(canvasPixels(canvas).data, ptsUs);
  }

  /**
   * Calls `draw` at the stream's frame rate and sends the canvas after
   * each call, until `stop()` or the end of the session.
   */
  drawLoop(canvas: CanvasLike, draw: (ctx: Context2DLike, timeMs: number) => void): { stop(): void } {
    const raw = this.#raw();
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new TvpipeError("invalid-argument", "the canvas has no 2D context");
    const interval = 1000 / raw.fps;
    const start = performance.now();
    let n = 0;
    let timer: NodeJS.Timeout | undefined;
    let running = true;
    const stop = () => {
      running = false;
      if (timer) clearTimeout(timer);
      this.#loops.delete(stop);
    };
    const tick = () => {
      if (!running || !this.proc.inputOpen) return stop();
      const t = performance.now() - start;
      try {
        draw(ctx, t);
        this.writeCanvas(canvas);
      } catch (err) {
        stop();
        this.emit("error", err instanceof TvpipeError ? err : new TvpipeError("invalid-argument", String(err), { cause: err }));
        return;
      }
      n++;
      // Skip ticks the drawing could not keep up with.
      const next = Math.max(n, Math.floor((performance.now() - start) / interval) + 1);
      n = next;
      timer = setTimeout(tick, Math.max(0, start + next * interval - performance.now()));
    };
    this.#loops.add(stop);
    timer = setTimeout(tick, 0);
    return { stop };
  }

  /**
   * One H.264 access unit (all NAL units of one picture; AVCC with 4-byte
   * lengths, or Annex B with start codes, detected unless `annexB` says).
   * Never dropped; `false` means the pipe is backed up (await `drain()`).
   */
  writeH264(au: ArrayBufferView | ArrayBuffer, options: { keyframe: boolean; ptsUs?: Pts; annexB?: boolean }): boolean {
    this.#encoded();
    if (options.keyframe) this.#keyframePending = false;
    const data = slrw.bytes(au);
    const annexB = options.annexB ?? slrw.isAnnexB(data);
    const flags = (options.keyframe ? slrw.H264_KEYFRAME : 0) | (annexB ? slrw.H264_ANNEX_B : 0);
    return this.proc.write(slrw.record(slrw.Kind.h264, this.#pts("video", options.ptsUs), data, flags));
  }

  /** The decoder configuration: an avcC record, or Annex B SPS + PPS. */
  writeH264Config(config: ArrayBufferView | ArrayBuffer, format: "avcc" | "annexb" = "avcc"): boolean {
    this.#encoded();
    const data = slrw.bytes(config);
    this.#config = data.slice();
    return this.proc.write(
      slrw.record(slrw.Kind.h264Config, 0, data, format === "avcc" ? slrw.CONFIG_AVCC : slrw.CONFIG_ANNEX_B),
    );
  }

  /** A WebCodecs `VideoEncoder` output: `encoder = new VideoEncoder({ output: (c, m) => s.writeChunk(c, m) })`. */
  writeChunk(chunk: EncodedVideoChunkLike, metadata?: EncodedVideoChunkMetadataLike): boolean {
    this.#encoded();
    const desc = metadata?.decoderConfig?.description;
    if (desc) {
      const d = slrw.bytes(desc);
      if (!this.#config || !sameBytes(this.#config, d)) this.writeH264Config(d, "avcc");
    }
    const data = new Uint8Array(chunk.byteLength);
    chunk.copyTo(data);
    return this.writeH264(data, { keyframe: chunk.type === "key", ptsUs: Math.max(0, chunk.timestamp) });
  }

  /** Resolves once the pipe took everything written. */
  drain(): Promise<void> {
    return this.proc.drain();
  }

  /** Ends the stream: what was written plays out, then the teardown. */
  end(): Promise<EndInfo> {
    for (const stop of this.#loops) stop();
    return this.proc.stop(5000 + this.#latencyMs, "end").then(() => this.done);
  }
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  if (a.byteLength !== b.byteLength) return false;
  for (let i = 0; i < a.byteLength; i++) if (a[i] !== b[i]) return false;
  return true;
}
