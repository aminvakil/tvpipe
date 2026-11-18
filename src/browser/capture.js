// tvpipe capture script (decision 0012, protocol: docs/share-protocol.md).
//
// Connects to tvpipe's WebSocket, captures a MediaStream (by default with
// getDisplayMedia), encodes the video as H.264 with WebCodecs and turns the
// audio into s16le PCM with an AudioWorklet, and streams both as SLRW
// records. Self-contained (no imports, no external resources), so tvpipe's
// share page uses it as its engine and the headless-browser mode can inject
// it into any page:
//
//   const c = tvpipeCapture.connect('ws://127.0.0.1:PORT/ws?t=TOKEN', opts);
//   c.on('status', s => ...);            // also: config, plan, stats, end, error, close
//   await c.captureDisplay();            // inside a user gesture (or auto-accepted)
//   // or: await c.share(mediaStream);
//   c.stop('reason');
//
// opts: preferCurrentTab, selfBrowserSurface ('exclude' by default),
// localAudio (keep playing captured tab audio locally; default false),
// profiles (H.264 profile order, e.g. ['42E0', '4D40', '6400']).
(function (global) {
  'use strict';
  if (global.tvpipeCapture) return;

  var VERSION = 1;
  var KIND_AUDIO = 2, KIND_H264 = 3, KIND_CONFIG = 4;
  var FLAG_KEY = 1, FLAG_ANNEXB = 2;
  // Bytes waiting in the socket before new frames are dropped (tvpipe is
  // behind; dropping before the encoder keeps the stream decodable).
  var MAX_BUFFERED = 4 << 20;
  // Without new frames (static content), the last one is encoded again
  // this often, so receivers keep getting pictures and keyframes.
  var IDLE_REPEAT_MS = 250;
  var STATS_MS = 2000;
  // Encoder warm-up: keyframes later than this are discarded (and asked
  // for again) until one is on time, for at most WARMUP_MAX_MS.
  var WARMUP_LATE_MS = 150;
  var WARMUP_MAX_MS = 3000;
  // Constrained Baseline first: no B-frames anywhere, every encoder has it.
  var DEFAULT_PROFILES = ['42E0', '4D40', '6400'];
  // [level_idc, MaxFS (macroblocks), MaxMBPS] (H.264 Table A-1).
  var LEVELS = [
    [0x1e, 1620, 40500], [0x1f, 3600, 108000], [0x20, 5120, 216000],
    [0x28, 8192, 245760], [0x2a, 8704, 522240], [0x32, 22080, 589824],
    [0x33, 36864, 983040], [0x34, 36864, 2073600]
  ];

  // The media clock: µs on performance.now(), for audio and video alike.
  function clock() { return Math.round(performance.now() * 1000); }

  function hex2(n) { return (n < 16 ? '0' : '') + n.toString(16).toUpperCase(); }

  function avcLevel(w, h, fps) {
    var fs = Math.ceil(w / 16) * Math.ceil(h / 16);
    var mbps = fs * fps;
    for (var i = 0; i < LEVELS.length; i++) {
      if (fs <= LEVELS[i][1] && mbps <= LEVELS[i][2]) return LEVELS[i][0];
    }
    return 0x34;
  }

  // One SLRW record: u8 kind | u64le pts_us | u32le len | payload parts.
  function record(kind, pts, parts) {
    var len = 0, i;
    for (i = 0; i < parts.length; i++) len += parts[i].byteLength;
    var buf = new Uint8Array(13 + len);
    var dv = new DataView(buf.buffer);
    dv.setUint8(0, kind);
    pts = Math.max(0, Math.round(pts));
    dv.setUint32(1, pts % 4294967296, true);
    dv.setUint32(5, Math.floor(pts / 4294967296), true);
    dv.setUint32(9, len, true);
    var off = 13;
    for (i = 0; i < parts.length; i++) {
      buf.set(parts[i], off);
      off += parts[i].byteLength;
    }
    return buf.buffer;
  }

  function bytesOf(src) {
    if (src instanceof ArrayBuffer) return new Uint8Array(src.slice(0));
    return new Uint8Array(src.buffer.slice(src.byteOffset, src.byteOffset + src.byteLength));
  }

  function sameBytes(a, b) {
    if (!a || !b || a.length !== b.length) return false;
    for (var i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    return true;
  }

  // Resamples (linearly) to the plan rate, converts to s16le interleaved,
  // and posts ~10 ms chunks. Outputs stay silent.
  var WORKLET = [
    'class TvpipePcm extends AudioWorkletProcessor {',
    '  constructor(o) {',
    '    super();',
    '    var p = o.processorOptions;',
    '    this.ch = p.channels; this.chunk = p.chunk;',
    '    this.step = sampleRate / p.rate; this.t = 0;',
    '    this.prev = new Float32Array(this.ch);',
    '    this.out = new Int16Array(this.chunk * this.ch); this.n = 0;',
    '    this.on = true;',
    '    this.port.onmessage = (e) => { if (e.data === "stop") this.on = false; };',
    '  }',
    '  process(inputs) {',
    '    if (!this.on) return false;',
    '    var inp = inputs[0] || [];',
    '    var frames = inp.length ? inp[0].length : 128;',
    '    var ch = this.ch, prev = this.prev;',
    '    var s = function (c, k) {',
    '      if (!inp.length) return 0;',
    '      return k < 0 ? prev[c] : inp[Math.min(c, inp.length - 1)][k];',
    '    };',
    '    while (this.t < frames - 1) {',
    '      var i = Math.floor(this.t), f = this.t - i;',
    '      for (var c = 0; c < ch; c++) {',
    '        var v = s(c, i) + (s(c, i + 1) - s(c, i)) * f;',
    '        v = v < -1 ? -1 : v > 1 ? 1 : v;',
    '        this.out[this.n * ch + c] = Math.round(v * 32767);',
    '      }',
    '      if (++this.n === this.chunk) {',
    '        this.port.postMessage(this.out.buffer, [this.out.buffer]);',
    '        this.out = new Int16Array(this.chunk * ch); this.n = 0;',
    '      }',
    '      this.t += this.step;',
    '    }',
    '    this.t -= frames;',
    '    for (var c2 = 0; c2 < ch; c2++) prev[c2] = s(c2, frames - 1);',
    '    return true;',
    '  }',
    '}',
    'registerProcessor("tvpipe-pcm", TvpipePcm);'
  ].join('\n');

  // Background tabs throttle timers (to 1/s, later 1/min), and the share
  // page is in the background while another tab is shared. Ticks from a
  // worker arrive as messages, which are not throttled; plain intervals
  // are the fallback (e.g. a page whose CSP forbids blob workers).
  var TICKER = 'var t=null;onmessage=function(e){clearInterval(t);if(e.data>0)t=setInterval(function(){postMessage(0)},e.data)}';
  function every(ms, fn) {
    var w = null, id = null;
    var fallback = function () {
      if (w) { w.terminate(); w = null; }
      if (id === null) id = setInterval(fn, ms);
    };
    try {
      var url = URL.createObjectURL(new Blob([TICKER], { type: 'text/javascript' }));
      w = new Worker(url);
      URL.revokeObjectURL(url);
      w.onmessage = fn;
      w.onerror = fallback;
      w.postMessage(ms);
    } catch (e) {
      fallback();
    }
    return function () {
      if (w) w.terminate();
      if (id !== null) clearInterval(id);
      w = null;
      id = null;
    };
  }

  function Capture(url, opts) {
    this.url = url;
    this.opts = opts || {};
    this.handlers = {};
    this.config = null;
    this.plan = null;
    this.stream = null;
    this.encoder = null;
    this.audioCtx = null;
    this.audioNode = null;
    this.lastFrame = null;
    this.lastEncodeAt = -1e12;
    this.needKey = true;
    this.sinceKey = 0;
    this.lastDesc = null;
    this.lastChunkTs = -1;
    this.timers = [];
    this.stopped = false;
    this.codec = '';
    this.stats = this._freshStats();
    this.ws = new WebSocket(url);
    this.ws.binaryType = 'arraybuffer';
    var self = this;
    this.ws.onopen = function () {
      self._send({
        type: 'hello', version: VERSION, clock: clock(), agent: navigator.userAgent,
        features: Capture.features()
      });
      self._emit('open');
    };
    this.ws.onmessage = function (e) {
      if (typeof e.data !== 'string') return;
      var m;
      try { m = JSON.parse(e.data); } catch (err) { return; }
      self._message(m);
    };
    this.ws.onclose = function (e) {
      self._teardown();
      self._emit('close', { code: e.code, reason: e.reason });
    };
  }

  // What this browser can do.
  Capture.features = function () {
    return {
      secure: !!global.isSecureContext,
      displayMedia: !!(navigator.mediaDevices && navigator.mediaDevices.getDisplayMedia),
      webcodecs: typeof global.VideoEncoder === 'function',
      trackProcessor: typeof global.MediaStreamTrackProcessor === 'function',
      audioWorklet: typeof global.AudioWorkletNode === 'function'
    };
  };

  Capture.prototype.on = function (ev, fn) {
    (this.handlers[ev] = this.handlers[ev] || []).push(fn);
    return this;
  };

  Capture.prototype._emit = function (ev, data) {
    var hs = this.handlers[ev] || [];
    for (var i = 0; i < hs.length; i++) {
      try { hs[i](data); } catch (e) { /* a UI error must not stop the stream */ }
    }
  };

  Capture.prototype._send = function (obj) {
    if (this.ws.readyState === 1) this.ws.send(JSON.stringify(obj));
  };

  Capture.prototype._log = function (level, message) {
    this._send({ type: 'log', level: level, message: String(message) });
    this._emit('log', { level: level, message: String(message) });
  };

  Capture.prototype._fail = function (message) {
    this._log('error', message);
    this._emit('error', String(message));
    this.stop(String(message));
  };

  Capture.prototype._message = function (m) {
    switch (m.type) {
      case 'config':
        this.config = m;
        this._emit('config', m);
        break;
      case 'status':
        this._emit('status', m);
        break;
      case 'plan':
        this.plan = m;
        this._emit('plan', m);
        var self = this;
        this._start(m).catch(function (e) { self._fail('cannot start: ' + (e && e.message || e)); });
        break;
      case 'keyframe':
        this._keyframe();
        break;
      case 'busy':
        this._emit('error', 'another page is already sharing to tvpipe');
        this._teardown();
        break;
      case 'end':
        this._teardown();
        this._emit('end', m.reason || 'tvpipe stopped');
        break;
    }
  };

  // Creates the AudioContext while the user gesture is still fresh (it
  // may start suspended otherwise).
  Capture.prototype._prepareAudio = function () {
    if (this.audioCtx || !this.config || !this.config.audio) return;
    var Ctx = global.AudioContext || global.webkitAudioContext;
    if (!Ctx) return;
    try {
      this.audioCtx = new Ctx({ sampleRate: this.config.sampleRate, latencyHint: 'interactive' });
    } catch (e) {
      try { this.audioCtx = new Ctx(); } catch (e2) { this.audioCtx = null; }
    }
    if (this.audioCtx && this.audioCtx.state !== 'running') this.audioCtx.resume().catch(function () {});
  };

  // getDisplayMedia with the constraints tvpipe asked for, then share().
  Capture.prototype.captureDisplay = function (extra) {
    if (!navigator.mediaDevices || !navigator.mediaDevices.getDisplayMedia) {
      return Promise.reject(new Error('this browser cannot capture the screen (getDisplayMedia)'));
    }
    var cfg = this.config || { fps: 30, audio: true };
    var o = this.opts;
    var video = { frameRate: { ideal: cfg.fps, max: cfg.fps } };
    var audio = cfg.audio ? {
      echoCancellation: false, noiseSuppression: false, autoGainControl: false,
      channelCount: { ideal: 2 },
      suppressLocalAudioPlayback: !o.localAudio
    } : false;
    var c = {
      video: video, audio: audio,
      selfBrowserSurface: o.selfBrowserSurface || 'exclude',
      surfaceSwitching: 'include', systemAudio: 'include'
    };
    if (o.preferCurrentTab) c.preferCurrentTab = true;
    for (var k in (extra || {})) c[k] = extra[k];
    this._prepareAudio();
    var md = navigator.mediaDevices, self = this;
    return md.getDisplayMedia(c).catch(function (e) {
      // Browsers without display audio may refuse the audio constraint.
      if (audio && e && (e.name === 'TypeError' || e.name === 'NotSupportedError' || e.name === 'OverconstrainedError')) {
        c.audio = false;
        return md.getDisplayMedia(c);
      }
      throw e;
    }).then(function (stream) { return self.share(stream); });
  };

  // Streams `stream` (its first video track and first audio track).
  Capture.prototype.share = function (stream) {
    if (this.stream) return Promise.reject(new Error('already sharing'));
    if (typeof global.VideoEncoder !== 'function') {
      stream.getTracks().forEach(function (t) { t.stop(); });
      return Promise.reject(new Error('this browser has no WebCodecs VideoEncoder'));
    }
    var vt = stream.getVideoTracks()[0];
    if (!vt) return Promise.reject(new Error('the stream has no video'));
    this.stream = stream;
    var self = this;
    stream.getTracks().forEach(function (t) {
      t.addEventListener('ended', function () {
        if (t === vt || t.kind === 'video') self.stop('the share was stopped in the browser');
      });
    });
    var s = vt.getSettings ? vt.getSettings() : {};
    var at = stream.getAudioTracks()[0];
    this._send({
      type: 'capture',
      width: s.width || 0, height: s.height || 0, fps: s.frameRate || null,
      audio: !!at, surface: s.displaySurface || null, label: vt.label || null
    });
    this._emit('capture', { settings: s, audio: !!at });
    return Promise.resolve();
  };

  Capture.prototype._pickCodec = function (plan) {
    var lvl = hex2(avcLevel(plan.width, plan.height, plan.fps));
    var profiles = this.opts.profiles || DEFAULT_PROFILES;
    var base = {
      width: plan.width, height: plan.height, bitrate: plan.bitrate, framerate: plan.fps,
      latencyMode: 'realtime', avc: { format: 'avc' }
    };
    var i = 0;
    var next = function () {
      if (i >= profiles.length) return Promise.reject(new Error('this browser cannot encode H.264 at ' + plan.width + 'x' + plan.height));
      var cfg = Object.assign({}, base, { codec: 'avc1.' + profiles[i++] + lvl });
      return VideoEncoder.isConfigSupported(cfg).then(function (r) {
        return r && r.supported ? cfg : next();
      }, function () { return next(); });
    };
    return next();
  };

  Capture.prototype._start = function (plan) {
    if (!this.stream) return Promise.reject(new Error('plan before a capture'));
    var self = this;
    this.frameUs = 1e6 / (plan.fps || 30);
    return this._pickCodec(plan).then(function (cfg) {
      self.codec = cfg.codec;
      self.encoder = new VideoEncoder({
        output: function (chunk, md) { self._chunk(chunk, md); },
        error: function (e) { self._fail('video encoder: ' + (e && e.message || e)); }
      });
      self.encoder.configure(cfg);
      self._log('info', 'encoding ' + cfg.codec + ' ' + plan.width + 'x' + plan.height + '@' + plan.fps + ' ' + Math.round(plan.bitrate / 1000) + ' kbit/s');
      self._videoSource(self.stream.getVideoTracks()[0]);
      self.timers.push(every(IDLE_REPEAT_MS / 2, function () { self._idle(); }));
      self.timers.push(every(STATS_MS, function () { self._reportStats(); }));
      return self._startAudio(plan).catch(function (e) {
        self._log('warn', 'no audio: ' + (e && e.message || e));
      });
    });
  };

  Capture.prototype._videoSource = function (track) {
    var self = this;
    if (typeof global.MediaStreamTrackProcessor === 'function') {
      var reader = new global.MediaStreamTrackProcessor({ track: track }).readable.getReader();
      this.reader = reader;
      var pump = function () {
        reader.read().then(function (r) {
          if (r.done || self.stopped) { if (r.value) r.value.close(); return; }
          self._frame(r.value);
          pump();
        }, function () {});
      };
      pump();
      return;
    }
    // Fallback: a hidden <video> and VideoFrame(video) per new frame.
    var v = document.createElement('video');
    v.muted = true;
    v.playsInline = true;
    v.autoplay = true;
    v.srcObject = new MediaStream([track]);
    v.style.cssText = 'position:fixed;left:-10px;top:-10px;width:2px;height:2px;opacity:0;pointer-events:none';
    (document.body || document.documentElement).appendChild(v);
    this.videoEl = v;
    var grab = function () {
      if (self.stopped || v.readyState < 2 || !v.videoWidth) return;
      var f;
      try { f = new VideoFrame(v, { timestamp: clock() }); } catch (e) { return; }
      self._frame(f);
    };
    v.play().catch(function () {});
    if (typeof v.requestVideoFrameCallback === 'function') {
      var cb = function () { grab(); if (!self.stopped) v.requestVideoFrameCallback(cb); };
      v.requestVideoFrameCallback(cb);
    } else {
      this.timers.push(every(Math.max(5, Math.round(this.frameUs / 1000)), grab));
    }
  };

  // A new source frame (owned by us from here on).
  Capture.prototype._frame = function (frame) {
    if (this.stopped || !this.encoder) { frame.close(); return; }
    if (this.lastFrame) this.lastFrame.close();
    this.lastFrame = frame;
    this.stats.source++;
    var t = clock();
    // Pace to the plan's frame rate (sources may deliver more).
    if (t - this.lastEncodeAt < this.frameUs - 2000 && !this.needKey) return;
    this._encode(frame, t);
  };

  Capture.prototype._idle = function () {
    var t = clock();
    if (this.lastFrame && t - this.lastEncodeAt >= IDLE_REPEAT_MS * 1000) {
      this.stats.repeated++;
      this._encode(this.lastFrame, t);
    }
  };

  Capture.prototype._keyframe = function () {
    this.needKey = true;
    this.stats.keyRequests++;
    // Static content may send no frame for a while: encode one now.
    var t = clock();
    if (this.lastFrame && t - this.lastEncodeAt >= this.frameUs / 2) this._encode(this.lastFrame, t);
  };

  Capture.prototype._encode = function (src, t) {
    var enc = this.encoder, plan = this.plan;
    if (!enc || enc.state !== 'configured') return;
    if (this.ws.bufferedAmount > MAX_BUFFERED || enc.encodeQueueSize > 2) {
      this.stats.dropped++;
      return;
    }
    var frame;
    try {
      var w = src.displayWidth, h = src.displayHeight;
      frame = (w === plan.width && h === plan.height)
        ? new VideoFrame(src, { timestamp: t })
        : this._scaled(src, t);
    } catch (e) {
      this.stats.dropped++;
      return;
    }
    var key = this.needKey || this.sinceKey + 1 >= (plan.keyInterval || 30);
    try {
      enc.encode(frame, { keyFrame: key });
    } finally {
      frame.close();
    }
    if (key) { this.needKey = false; this.sinceKey = 0; } else { this.sinceKey++; }
    this.lastEncodeAt = t;
    this.stats.encoded++;
  };

  // Scales (letterboxed) into the plan size through a canvas.
  Capture.prototype._scaled = function (src, t) {
    var W = this.plan.width, H = this.plan.height;
    if (!this.canvas) {
      if (typeof global.OffscreenCanvas === 'function') {
        this.canvas = new OffscreenCanvas(W, H);
      } else {
        this.canvas = document.createElement('canvas');
        this.canvas.width = W;
        this.canvas.height = H;
      }
      this.ctx2d = this.canvas.getContext('2d', { alpha: false, desynchronized: true });
      this.ctx2d.imageSmoothingEnabled = true;
      this.ctx2d.imageSmoothingQuality = 'high';
    }
    var sw = src.displayWidth, sh = src.displayHeight;
    var k = Math.min(W / sw, H / sh);
    var dw = Math.round(sw * k), dh = Math.round(sh * k);
    var dx = Math.floor((W - dw) / 2), dy = Math.floor((H - dh) / 2);
    var g = this.ctx2d;
    if (dx > 0 || dy > 0) {
      g.fillStyle = '#000';
      g.fillRect(0, 0, W, H);
    }
    g.drawImage(src, dx, dy, dw, dh);
    return new VideoFrame(this.canvas, { timestamp: t, alpha: 'discard' });
  };

  Capture.prototype._chunk = function (chunk, md) {
    if (this.stopped || this.ws.readyState !== 1) return;
    var cfg = md && md.decoderConfig;
    if (cfg && cfg.description) {
      var desc = bytesOf(cfg.description);
      if (!sameBytes(desc, this.lastDesc)) {
        this.lastDesc = desc;
        this.ws.send(record(KIND_CONFIG, chunk.timestamp, [new Uint8Array([0]), desc]));
      }
    }
    var late = (clock() - chunk.timestamp) / 1000;
    if (!this.sentKey) {
      // The first pictures out of a (hardware) encoder can be most of a
      // second old: starting with them would put tvpipe's timeline that
      // far behind (Cast's in-flight window overflows at once). Wait for
      // a keyframe that is on time, unless the encoder never gets there.
      if (this.firstChunkAt === undefined) this.firstChunkAt = clock();
      var patient = clock() - this.firstChunkAt < WARMUP_MAX_MS * 1000;
      if (chunk.type !== 'key' || (late > WARMUP_LATE_MS && patient)) {
        if (chunk.type === 'key') this.needKey = true;
        this.stats.warmup++;
        return;
      }
    }
    var data = new Uint8Array(chunk.byteLength);
    chunk.copyTo(data);
    var flags = chunk.type === 'key' ? FLAG_KEY : 0;
    // Without an avcC the encoder made Annex B (parameter sets in band).
    if (!this.lastDesc && data.length > 4 && data[0] === 0 && data[1] === 0 &&
        (data[2] === 1 || (data[2] === 0 && data[3] === 1))) flags |= FLAG_ANNEXB;
    if (chunk.timestamp <= this.lastChunkTs && !this.warnedOrder) {
      this.warnedOrder = true;
      this._log('warn', 'the encoder reorders frames (B-frames?)');
    }
    this.lastChunkTs = chunk.timestamp;
    this.ws.send(record(KIND_H264, chunk.timestamp, [new Uint8Array([flags]), data]));
    if (flags & FLAG_KEY) this.sentKey = true;
    this.stats.bytes += data.length;
    this.stats.chunks++;
    this.stats.latencySum += late;
  };

  Capture.prototype._startAudio = function (plan) {
    var track = this.stream.getAudioTracks()[0];
    if (!track || !plan.audio) return Promise.resolve();
    if (typeof global.AudioWorkletNode !== 'function') return Promise.reject(new Error('no AudioWorklet'));
    if (!this.audioCtx) this._prepareAudio();
    if (!this.audioCtx) return Promise.reject(new Error('no AudioContext'));
    var self = this, rate = plan.audio.sampleRate, ch = plan.audio.channels;
    var url = URL.createObjectURL(new Blob([WORKLET], { type: 'text/javascript' }));
    var connect = function () {
      var ctx = self.audioCtx;
      var src = ctx.createMediaStreamSource(new MediaStream([track]));
      var node = new AudioWorkletNode(ctx, 'tvpipe-pcm', {
        numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1],
        processorOptions: { rate: rate, channels: ch, chunk: Math.round(rate / 100) }
      });
      node.port.onmessage = function (e) {
        // Nothing before the first keyframe went out (tvpipe's timeline
        // starts there).
        if (self.stopped || self.ws.readyState !== 1 || !self.sentKey) return;
        var pcm = new Uint8Array(e.data);
        var dur = pcm.length / (2 * ch) / rate * 1e6;
        if (self.ws.bufferedAmount > MAX_BUFFERED) { self.stats.audioDropped++; return; }
        self.ws.send(record(KIND_AUDIO, clock() - dur, [pcm]));
        self.stats.audioBytes += pcm.length;
      };
      src.connect(node);
      node.connect(ctx.destination);
      self.audioNode = node;
      self.audioSrc = src;
      if (ctx.state !== 'running') ctx.resume().catch(function () {});
      self._log('info', 'audio: ' + ctx.sampleRate + ' Hz context, sent as ' + rate + ' Hz ' + ch + ' ch');
    };
    var load = function () { return self.audioCtx.audioWorklet.addModule(url); };
    return load().then(function () {
      try {
        connect();
      } catch (e) {
        // Some browsers refuse a MediaStream source at a rate other than
        // the track's: use the default rate; the worklet resamples.
        self.audioCtx.close().catch(function () {});
        self.audioCtx = new (global.AudioContext || global.webkitAudioContext)();
        return load().then(connect);
      }
    }).then(function () { URL.revokeObjectURL(url); });
  };

  Capture.prototype._freshStats = function () {
    return {
      since: performance.now(), source: 0, encoded: 0, repeated: 0, dropped: 0, chunks: 0,
      bytes: 0, latencySum: 0, keyRequests: 0, audioBytes: 0, audioDropped: 0, warmup: 0
    };
  };

  Capture.prototype._reportStats = function () {
    var s = this.stats, secs = (performance.now() - s.since) / 1000;
    if (secs <= 0) return;
    var out = {
      type: 'stats',
      fps: +(s.chunks / secs).toFixed(1),
      sourceFps: +(s.source / secs).toFixed(1),
      kbps: Math.round(s.bytes * 8 / secs / 1000),
      audioKbps: Math.round(s.audioBytes * 8 / secs / 1000),
      encodeMs: s.chunks ? +(s.latencySum / s.chunks).toFixed(1) : null,
      dropped: s.dropped, repeated: s.repeated, keyRequests: s.keyRequests, warmup: s.warmup,
      queue: this.encoder ? this.encoder.encodeQueueSize : 0,
      buffered: this.ws.bufferedAmount, codec: this.codec,
      width: this.plan ? this.plan.width : 0, height: this.plan ? this.plan.height : 0
    };
    this._send(out);
    this._emit('stats', out);
    this.stats = this._freshStats();
  };

  // Ends the share: tells tvpipe, stops the capture.
  Capture.prototype.stop = function (reason) {
    if (this.stopped) return;
    this._send({ type: 'ended', reason: reason || 'the page stopped sharing' });
    this._teardown();
    var ws = this.ws;
    setTimeout(function () { try { ws.close(1000, 'ended'); } catch (e) { /* closed */ } }, 100);
    this._emit('end', reason || 'stopped');
  };

  Capture.prototype._teardown = function () {
    if (this.stopped) return;
    this.stopped = true;
    this.timers.forEach(function (cancel) { cancel(); });
    this.timers = [];
    if (this.reader) this.reader.cancel().catch(function () {});
    if (this.stream) this.stream.getTracks().forEach(function (t) { t.stop(); });
    if (this.encoder && this.encoder.state !== 'closed') { try { this.encoder.close(); } catch (e) { /* closed */ } }
    if (this.lastFrame) { this.lastFrame.close(); this.lastFrame = null; }
    if (this.audioNode) { this.audioNode.port.postMessage('stop'); this.audioNode.disconnect(); }
    if (this.audioSrc) this.audioSrc.disconnect();
    if (this.audioCtx) this.audioCtx.close().catch(function () {});
    if (this.videoEl) this.videoEl.remove();
  };

  global.tvpipeCapture = {
    version: VERSION,
    features: Capture.features,
    connect: function (url, opts) { return new Capture(url, opts); }
  };
})(typeof globalThis !== 'undefined' ? globalThis : window);
