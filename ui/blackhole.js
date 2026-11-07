// Kakkoii's orb: a small black hole. A black event horizon with a thin photon
// ring, an accretion disk seen nearly edge-on (the near half crosses in front
// of the hole, the far half is bent over the top and under the bottom, as in
// Interstellar), the approaching side brighter. White when idle; red-hot while
// recording; amber and still when paused. ~30 fps idle, 60 while you use it.

const FRAG = `
precision highp float;
uniform vec2 res;
uniform float t, heat, held, glow;
float h(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float n(vec2 p) { vec2 i = floor(p), f = fract(p); f = f * f * (3. - 2. * f);
  return mix(mix(h(i), h(i + vec2(1, 0)), f.x), mix(h(i + vec2(0, 1)), h(i + vec2(1, 1)), f.x), f.y); }
float fbm(vec2 p) { float v = 0., a = .5; for (int i = 0; i < 4; i++) { v += a * n(p); p = p * 2.03 + 7.1; a *= .5; } return v; }

const float RH = .34;            // the horizon
const float RIN = .43, ROUT = .97; // the disk
// Brightness of the disk at radius r, angle a (in its own plane). Inner parts turn faster.
float disk(float r, float a) {
  if (r < RIN || r > ROUT) return 0.;
  float k = (r - RIN) / (ROUT - RIN);
  float base = pow(1. - k, 1.7) * smoothstep(0., .06, k) * smoothstep(1., .82, k);
  float spin = a - t * 1.6 / pow(r, 1.5);
  vec2 q = r * vec2(cos(spin), sin(spin));
  float gas = fbm(q * 3.2 + r * 2.) * .8 + fbm(vec2(r * 14., spin * 1.3)) * .35;
  return base * (.35 + gas);
}
void main() {
  vec2 uv = (gl_FragCoord.xy - .5 * res) / (.5 * res.y);
  float R = length(uv), ang = atan(uv.y, uv.x);
  float tilt = .22;

  // The far side of the disk, bent round the hole into a halo.
  float lr = (R - RH * 1.02) / (RH * .55);
  float halo = 0.;
  if (lr > 0. && lr < 1.) halo = disk(RIN + lr * (ROUT - RIN) * .5, ang + 3.14159) * (.25 + .75 * pow(abs(sin(ang)), 1.5)) * pow(1. - lr, 1.6) * 2.1;
  // The photon ring.
  float ring = exp(-pow((R - RH * 1.04) / .013, 2.)) * (1.2 + .3 * sin(ang * 2. + t));

  // The disk itself, nearly edge-on.
  vec2 dp = vec2(uv.x, uv.y / tilt);
  float dr = length(dp), da = atan(dp.y, dp.x);
  float dop = 1. - .55 * dp.x / max(dr, .01); // the side coming at you is brighter
  float d = disk(dr, da) * dop * 1.5;
  bool near = uv.y < 0.;

  float hole = 1. - smoothstep(RH - .012, RH + .004, R);
  float back = (halo + ring + (near ? 0. : d)) * (1. - hole);
  float L = back + (near ? d : 0.);
  L *= .8 + .45 * glow;

  vec3 cool = vec3(1., .97, .93);
  vec3 hot = mix(vec3(1., .2, .14), vec3(1., .62, .38), clamp(L - .6, 0., 1.));
  vec3 amber = vec3(.98, .75, .2);
  vec3 tint = mix(mix(cool, hot, heat), amber, held);
  vec3 col = 1. - exp(-tint * L * 1.7);
  // A soft dark well round the hole, so it reads on a white page too.
  float shade = (1. - smoothstep(RH, RH * 1.45, R)) * .5;
  float a = clamp(max(max(max(col.r, col.g), col.b), max(hole, shade)), 0., 1.);
  gl_FragColor = vec4(min(col, vec3(a)), a);
}`;

const VERT = 'attribute vec2 p; void main() { gl_Position = vec4(p, 0., 1.); }';

export function blackHole(canvas) {
  const gl = canvas.getContext('webgl', { premultipliedAlpha: true, alpha: true, antialias: false });
  const st = { heat: 0, held: 0, glow: 0, want: { heat: 0, held: 0, glow: 0 }, fast: false, t: Math.random() * 100 };
  if (!gl) { canvas.classList.add('flat'); return { set() {} }; }
  const sh = (type, src) => { const s = gl.createShader(type); gl.shaderSource(s, src); gl.compileShader(s); return s; };
  const prog = gl.createProgram();
  gl.attachShader(prog, sh(gl.VERTEX_SHADER, VERT));
  gl.attachShader(prog, sh(gl.FRAGMENT_SHADER, FRAG));
  gl.linkProgram(prog);
  gl.useProgram(prog);
  gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog, 'p');
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  const u = (k) => gl.getUniformLocation(prog, k);
  const U = { res: u('res'), t: u('t'), heat: u('heat'), held: u('held'), glow: u('glow') };

  const size = () => {
    const d = Math.min(devicePixelRatio || 1, 2);
    canvas.width = Math.round(canvas.clientWidth * d);
    canvas.height = Math.round(canvas.clientHeight * d);
    gl.viewport(0, 0, canvas.width, canvas.height);
  };
  size();

  let last = performance.now(), drawn = 0;
  const frame = (now) => {
    requestAnimationFrame(frame);
    if (document.hidden) return;
    // Idle: 30 frames a second is plenty for something this slow.
    if (!st.fast && now - drawn < 32) return;
    const dt = Math.min(0.1, (now - last) / 1000);
    last = now; drawn = now;
    for (const k of ['heat', 'held', 'glow']) st[k] += (st.want[k] - st[k]) * Math.min(1, dt * 4);
    // Paused: the disk slows to a stop. Recording: it runs faster.
    st.t += dt * (1 - 0.95 * st.held) * (1 + 1.2 * st.heat);
    gl.uniform2f(U.res, canvas.width, canvas.height);
    gl.uniform1f(U.t, st.t);
    gl.uniform1f(U.heat, st.heat);
    gl.uniform1f(U.held, st.held);
    gl.uniform1f(U.glow, st.glow);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  };
  requestAnimationFrame(frame);

  return {
    set({ recording, paused, active } = {}) {
      if (recording !== undefined) st.want.heat = recording ? 1 : 0;
      if (paused !== undefined) st.want.held = paused ? 1 : 0;
      if (active !== undefined) { st.want.glow = active ? 1 : 0; st.fast = !!active; }
    },
    resize: size,
  };
}
