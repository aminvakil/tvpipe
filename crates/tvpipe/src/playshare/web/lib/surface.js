/**
 * Surfaces (docs/playshare-design.md §17): a canvas the page draws is
 * encoded here with WebCodecs and streamed by tvpipe to TVs, without a
 * browser of tvpipe's capturing a tab. One surface per screen: different
 * canvases on different TVs.
 *
 *   const s = await tvpipe.surface({ canvas, to: ["Living Room"], fps: 30 });
 *   function draw() { render(ctx); s.frame(); requestAnimationFrame(draw); }   // or clock: "timer"
 *
 * The encoder side runs in a window or a worker (an OffscreenCanvas
 * there): `SurfaceEncoder` needs only `WebSocket`, `VideoEncoder` and
 * `VideoFrame`. Sound: `captureAudio()` on the main thread hands PCM to
 * `pushAudio()`.
 */
import { Emitter } from "./emitter.js";
/** The wall clock in µs, the clock of every record (tvpipe measures the lag with it). */
export function wallMicros() {
    return Math.round((performance.timeOrigin + performance.now()) * 1000);
}
/** One SLRW record: `u8 kind | u64le pts_us | u32le len | payload`. */
export function slrwRecord(kind, ptsUs, payload, prefix) {
    const extra = prefix === undefined ? 0 : 1;
    const out = new Uint8Array(13 + extra + payload.length);
    const dv = new DataView(out.buffer);
    dv.setUint8(0, kind);
    dv.setBigUint64(1, BigInt(Math.max(0, Math.round(ptsUs))), true);
    dv.setUint32(9, payload.length + extra, true);
    if (prefix !== undefined)
        out[13] = prefix;
    out.set(payload, 13 + extra);
    return out;
}
/** H.264 profiles and levels to try, the smallest level that fits first. */
function codecs(width, height, fps) {
    const mbs = Math.ceil(width / 16) * Math.ceil(height / 16);
    const rate = mbs * fps;
    // Levels 3.1, 4.0, 4.2, 5.1 (MaxFS, MaxMBPS).
    const levels = [
        ["1F", 3600, 108000],
        ["28", 8192, 245760],
        ["2A", 8704, 522240],
        ["33", 36864, 983040],
    ];
    const level = (levels.find(([, fs, mbps]) => mbs <= fs && rate <= mbps) ?? levels[3])[0];
    // Constrained Baseline first (no B-frames anywhere), then Main and High.
    return [`avc1.42E0${level}`, `avc1.4D40${level}`, `avc1.6400${level}`];
}
/**
 * Encodes a canvas and sends it to tvpipe's surface socket. Works in a
 * window or a worker.
 */
export class SurfaceEncoder extends Emitter {
    info;
    canvas;
    #ws;
    #encoder;
    #keyInterval;
    #lastKey = -Infinity;
    #wantKey = true;
    #config;
    #timer;
    #lastFrame = -Infinity;
    #closed = false;
    #skipped = 0;
    #encodeMs = 0;
    #encoded = 0;
    #pending = new Map();
    /** Until the first keyframe went out: the encoder warming up. */
    #warm = false;
    #warmUntil = 0;
    #ready;
    /** The codec in use (`avc1.42E01F`…), once configured. */
    codec = "";
    constructor(info, canvas, opts = {}) {
        super();
        this.info = info;
        this.canvas = canvas;
        this.#keyInterval = (opts.keyInterval ?? 2) * 1e6;
        const url = opts.url ??
            `${globalThis.location.protocol === "https:" ? "wss:" : "ws:"}//${globalThis.location.host}${info.path}`;
        this.#ws = new WebSocket(url);
        this.#ws.binaryType = "arraybuffer";
        this.#ws.onmessage = (ev) => {
            if (typeof ev.data !== "string")
                return;
            let m;
            try {
                m = JSON.parse(ev.data);
            }
            catch {
                return;
            }
            if (m["t"] === "keyframe")
                this.#wantKey = true;
            else if (m["t"] === "status")
                this.emit("status", m);
            else if (m["t"] === "stats") {
                const n = Math.max(1, this.#encoded);
                this.emit("stats", { ...m, encodeMs: Math.round((this.#encodeMs / n) * 10) / 10, skipped: this.#skipped });
                this.#encodeMs = 0;
                this.#encoded = 0;
                this.#skipped = 0;
            }
            else if (m["t"] === "end")
                this.close();
        };
        this.#ws.onclose = () => {
            if (!this.#closed)
                this.close();
        };
        const bitrate = opts.bitrate ?? Math.round(info.width * info.height * info.fps * 0.1);
        this.#ready = this.#configure(bitrate).then(() => new Promise((ok, fail) => {
            if (this.#ws.readyState === 1)
                return ok();
            this.#ws.onopen = () => ok();
            this.#ws.onerror = () => fail(new Error("cannot reach tvpipe's surface socket"));
        }));
        this.#ready.then(() => {
            this.#ws.send(JSON.stringify({ t: "log", codec: this.codec, agent: globalThis.navigator?.userAgent ?? "" }));
            if (opts.clock === "timer")
                this.#tick(performance.now());
        }, (e) => this.emit("error", e));
    }
    /** Resolves when the encoder and the socket are ready. */
    get ready() {
        return this.#ready;
    }
    async #configure(bitrate) {
        const { width, height, fps } = this.info;
        for (const accel of ["prefer-hardware", "no-preference"]) {
            for (const codec of codecs(width, height, fps)) {
                const config = {
                    codec,
                    width,
                    height,
                    bitrate,
                    framerate: fps,
                    latencyMode: "realtime",
                    hardwareAcceleration: accel,
                    avc: { format: "avc" },
                };
                let ok = false;
                try {
                    ok = (await VideoEncoder.isConfigSupported(config)).supported === true;
                }
                catch { }
                if (!ok)
                    continue;
                this.#encoder = new VideoEncoder({
                    output: (chunk, meta) => this.#output(chunk, meta),
                    error: (e) => this.emit("error", e instanceof Error ? e : new Error(String(e))),
                });
                this.#encoder.configure(config);
                this.codec = `${codec}${accel === "prefer-hardware" ? " (hw)" : ""}`;
                return;
            }
        }
        throw new Error("this browser cannot encode H.264 (VideoEncoder)");
    }
    #send(data) {
        if (this.#ws.readyState !== 1)
            return;
        this.#ws.send(data);
    }
    #output(chunk, meta) {
        const started = this.#pending.get(chunk.timestamp);
        this.#pending.delete(chunk.timestamp);
        if (started !== undefined) {
            this.#encodeMs += performance.now() - started;
            this.#encoded++;
        }
        const desc = meta?.decoderConfig?.description;
        if (desc) {
            const bytes = ArrayBuffer.isView(desc)
                ? new Uint8Array(desc.buffer, desc.byteOffset, desc.byteLength)
                : new Uint8Array(desc);
            const key = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
            if (key !== this.#config) {
                this.#config = key;
                this.#send(slrwRecord(4, chunk.timestamp, bytes, 0));
            }
        }
        // Warm-up (as capture.js, docs/share-protocol.md): a hardware encoder's
        // first pictures come out most of a second after their frames; a
        // stale first keyframe would make tvpipe's clock keep that delay.
        // Until a keyframe goes out, drop the others and keyframes more than
        // 150 ms old (and ask for another), for at most 3 s.
        if (!this.#warm) {
            const now = wallMicros();
            if (this.#warmUntil === 0)
                this.#warmUntil = now + 3e6;
            const fresh = now - chunk.timestamp <= 150_000 || now > this.#warmUntil;
            if (chunk.type !== "key" || !fresh) {
                if (chunk.type === "key")
                    this.#wantKey = true;
                return;
            }
            this.#warm = true;
        }
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        // Without an avcC the encoder wrote Annex B with its parameter sets in band.
        const flags = (chunk.type === "key" ? 1 : 0) | (this.#config === undefined ? 2 : 0);
        this.#send(slrwRecord(3, chunk.timestamp, data, flags));
    }
    /** The timer clock: a frame every 1/fps, catching up on late wake-ups. */
    #tick(due) {
        if (this.#closed)
            return;
        this.frame(true);
        const step = 1000 / this.info.fps;
        let next = due + step;
        const now = performance.now();
        if (next < now - step)
            next = now;
        this.#timer = setTimeout(() => this.#tick(next), Math.max(0, next - now));
    }
    /**
     * Encodes what the canvas shows now (call it after drawing; faster than
     * the surface's rate is paced down). Skipped while the encoder or the
     * socket are behind.
     */
    frame(force = false) {
        const enc = this.#encoder;
        if (this.#closed || !enc || enc.state !== "configured" || this.#ws.readyState !== 1)
            return;
        const now = performance.now();
        if (!force && now - this.#lastFrame < 850 / this.info.fps)
            return;
        this.#lastFrame = now;
        if (enc.encodeQueueSize > 2 || this.#ws.bufferedAmount > 4 << 20) {
            this.#skipped++;
            return;
        }
        const ts = wallMicros();
        const key = this.#wantKey || ts - this.#lastKey >= this.#keyInterval;
        let frame;
        try {
            frame = new VideoFrame(this.canvas, { timestamp: ts });
        }
        catch (e) {
            this.#skipped++;
            return;
        }
        this.#pending.set(ts, performance.now());
        if (this.#pending.size > 64)
            this.#pending.clear();
        enc.encode(frame, { keyFrame: key });
        frame.close();
        if (key) {
            this.#wantKey = false;
            this.#lastKey = ts;
        }
    }
    /** Interleaved s16 PCM at the surface's rate (`captureAudio` makes it). */
    pushAudio(pcm, ptsUs = wallMicros()) {
        if (this.#closed || !this.info.audio)
            return;
        this.#send(slrwRecord(2, ptsUs, new Uint8Array(pcm.buffer, pcm.byteOffset, pcm.byteLength)));
    }
    close() {
        if (this.#closed)
            return;
        this.#closed = true;
        if (this.#timer)
            clearTimeout(this.#timer);
        try {
            if (this.#encoder?.state === "configured")
                this.#encoder.close();
        }
        catch { }
        try {
            this.#ws.close();
        }
        catch { }
        this.emit("end");
    }
}
/** The AudioWorklet that hands its input on as Float32 blocks. */
const WORKLET = `class TvpipePcm extends AudioWorkletProcessor {
  process(inputs) {
    const ch = inputs[0];
    if (ch && ch.length) this.port.postMessage(ch.map((c) => c.slice(0)));
    return true;
  }
}
registerProcessor("tvpipe-pcm", TvpipePcm);`;
/**
 * Taps a Web Audio node and calls `onPcm` with ~10 ms of interleaved s16
 * PCM and its wall-clock time (µs), at the context's rate. The node still
 * plays wherever it is connected. Main thread only (AudioContext).
 */
export async function captureAudio(ctx, node, channels, onPcm) {
    const url = URL.createObjectURL(new Blob([WORKLET], { type: "text/javascript" }));
    await ctx.audioWorklet.addModule(url);
    URL.revokeObjectURL(url);
    const tap = new AudioWorkletNode(ctx, "tvpipe-pcm", { numberOfInputs: 1, numberOfOutputs: 0, channelCount: channels, channelCountMode: "explicit" });
    node.connect(tap);
    const per = Math.round(ctx.sampleRate / 100);
    let buf = new Int16Array(per * channels);
    let fill = 0;
    tap.port.onmessage = (ev) => {
        const blocks = ev.data;
        const n = blocks[0]?.length ?? 0;
        for (let i = 0; i < n; i++) {
            for (let c = 0; c < channels; c++) {
                const v = (blocks[c] ?? blocks[0])[i];
                buf[fill * channels + c] = Math.max(-32768, Math.min(32767, Math.round(v * 32767)));
            }
            if (++fill === per) {
                onPcm(buf, wallMicros() - 10_000);
                buf = new Int16Array(per * channels);
                fill = 0;
            }
        }
    };
    return () => {
        node.disconnect(tap);
        tap.port.onmessage = null;
    };
}
