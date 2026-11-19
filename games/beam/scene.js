// The demo's TV picture, for WebGL2 in a window, a worker (OffscreenCanvas)
// or the tab-capture page: an animated shader field, spinning instanced
// triangles, and a row of blocks with the wall clock in ms (the latency
// harness reads it back from the player page).

const VS_FIELD = `#version 300 es
in vec2 p; out vec2 uv; void main() { uv = p * 0.5 + 0.5; gl_Position = vec4(p, 0, 1); }`;
const FS_FIELD = `#version 300 es
precision highp float; in vec2 uv; out vec4 o; uniform float t; uniform vec3 tint;
void main() {
  vec2 q = uv * vec2(16.0, 9.0);
  float v = sin(q.x * 0.9 + t) + sin(q.y * 1.3 - t * 1.4) + sin((q.x + q.y) * 0.6 + t * 0.7) + sin(length(q - vec2(8.0, 4.5)) * 1.7 - t * 2.0);
  o = vec4(0.5 + 0.5 * cos(v + vec3(0.0, 2.0, 4.0) + tint), 1.0);
}`;
const VS_TRI = `#version 300 es
in vec2 p; in vec3 inst; uniform float t; uniform vec2 aspect; out vec3 col;
void main() {
  float a = t * (0.5 + fract(inst.z * 7.0)) + inst.z * 6.283;
  mat2 r = mat2(cos(a), sin(a), -sin(a), cos(a));
  vec2 pos = inst.xy + r * p * 0.06;
  gl_Position = vec4(pos * aspect, 0, 1);
  col = 0.5 + 0.5 * cos(inst.z * 12.0 + vec3(0, 2, 4));
}`;
const FS_TRI = `#version 300 es
precision mediump float; in vec3 col; out vec4 o; void main() { o = vec4(col, 1.0); }`;

/** Bits of the clock row: 2 start, 44 data (ms since the epoch), 2 end. */
export const CODE = { x: 16, y: 8, size: 20, gap: 2, bits: 48 };

function program(gl, vs, fs) {
  const p = gl.createProgram();
  for (const [type, src] of [[gl.VERTEX_SHADER, vs], [gl.FRAGMENT_SHADER, fs]]) {
    const s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
    gl.attachShader(p, s);
  }
  gl.linkProgram(p);
  return p;
}

/** @param {HTMLCanvasElement | OffscreenCanvas} canvas */
export function makeScene(canvas, { tint = [0, 0, 0], count = 400 } = {}) {
  const gl = canvas.getContext("webgl2", { antialias: true, preserveDrawingBuffer: true });
  if (!gl) throw new Error("no WebGL2");
  const field = program(gl, VS_FIELD, FS_FIELD);
  const tri = program(gl, VS_TRI, FS_TRI);
  const quad = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, quad);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
  const triBuf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, triBuf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, 1, -0.87, -0.5, 0.87, -0.5]), gl.STATIC_DRAW);
  const inst = new Float32Array(count * 3);
  for (let i = 0; i < count; i++) {
    inst[i * 3] = Math.random() * 3.4 - 1.7;
    inst[i * 3 + 1] = Math.random() * 1.9 - 0.95;
    inst[i * 3 + 2] = Math.random();
  }
  const instBuf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, instBuf);
  gl.bufferData(gl.ARRAY_BUFFER, inst, gl.STATIC_DRAW);
  const vaoField = gl.createVertexArray();
  gl.bindVertexArray(vaoField);
  gl.bindBuffer(gl.ARRAY_BUFFER, quad);
  const pf = gl.getAttribLocation(field, "p");
  gl.enableVertexAttribArray(pf);
  gl.vertexAttribPointer(pf, 2, gl.FLOAT, false, 0, 0);
  const vaoTri = gl.createVertexArray();
  gl.bindVertexArray(vaoTri);
  gl.bindBuffer(gl.ARRAY_BUFFER, triBuf);
  const pt = gl.getAttribLocation(tri, "p");
  gl.enableVertexAttribArray(pt);
  gl.vertexAttribPointer(pt, 2, gl.FLOAT, false, 0, 0);
  gl.bindBuffer(gl.ARRAY_BUFFER, instBuf);
  const pi = gl.getAttribLocation(tri, "inst");
  gl.enableVertexAttribArray(pi);
  gl.vertexAttribPointer(pi, 3, gl.FLOAT, false, 0, 0);
  gl.vertexAttribDivisor(pi, 1);
  const uT = gl.getUniformLocation(field, "t");
  const uTint = gl.getUniformLocation(field, "tint");
  const uT2 = gl.getUniformLocation(tri, "t");
  const uAspect = gl.getUniformLocation(tri, "aspect");

  return {
    /** Draws the picture for wall-clock time `now` (ms). */
    draw(now = Date.now()) {
      const w = canvas.width;
      const h = canvas.height;
      const t = (now % 600_000) / 1000;
      gl.viewport(0, 0, w, h);
      gl.disable(gl.SCISSOR_TEST);
      gl.useProgram(field);
      gl.uniform1f(uT, t);
      gl.uniform3fv(uTint, tint);
      gl.bindVertexArray(vaoField);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
      gl.useProgram(tri);
      gl.uniform1f(uT2, t);
      gl.uniform2f(uAspect, h / w, 1);
      gl.bindVertexArray(vaoTri);
      gl.drawArraysInstanced(gl.TRIANGLES, 0, 3, count);
      // The clock row.
      gl.enable(gl.SCISSOR_TEST);
      const { x, y, size, gap, bits } = CODE;
      gl.scissor(x - gap * 2, h - y - size - gap * 2, bits * (size + gap) + gap * 3, size + gap * 4);
      gl.clearColor(0.5, 0.5, 0.5, 1);
      gl.clear(gl.COLOR_BUFFER_BIT);
      const ms = Math.floor(now);
      for (let i = 0; i < bits; i++) {
        let on;
        if (i < 2) on = i === 0;
        else if (i >= bits - 2) on = i === bits - 1;
        else on = Math.floor(ms / 2 ** (bits - 3 - i)) % 2 === 1;
        gl.scissor(x + i * (size + gap), h - y - size, size, size);
        gl.clearColor(on ? 1 : 0, on ? 1 : 0, on ? 1 : 0, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);
      }
      gl.disable(gl.SCISSOR_TEST);
    },
  };
}

/**
 * Reads the clock row back from RGBA pixels of the row (`row`: the
 * pixels of the line through the blocks' middle, `width` wide, scaled by
 * `scale` from the drawn size). The ms, or null when it does not read.
 */
export function readCode(row, scale = 1) {
  const { x, size, gap, bits } = CODE;
  let ms = 0;
  for (let i = 0; i < bits; i++) {
    const px = Math.round((x + i * (size + gap) + size / 2) * scale);
    const o = px * 4;
    const lum = (row[o] + row[o + 1] + row[o + 2]) / 3;
    const on = lum > 128;
    if (lum > 80 && lum < 176) return null;
    if (i < 2) {
      if (on !== (i === 0)) return null;
    } else if (i >= bits - 2) {
      if (on !== (i === bits - 1)) return null;
    } else ms = ms * 2 + (on ? 1 : 0);
  }
  return ms;
}
