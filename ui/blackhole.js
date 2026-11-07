// Kakkoii's orb: a black hole inside a black glass lens. A sharp event horizon
// with a thin photon ring, a grooved disk seen nearly edge-on (the near half
// crosses in front of the hole, the far half is bent over the top and under the
// bottom, as in Interstellar), the approaching side brighter, and a few stars
// pulled into a ring. Silver when idle; red-hot while recording; amber and still
// when paused. ~30 fps idle, 60 while you use it.

const FRAG = `
precision highp float;
uniform vec2 res;
uniform float t, heat, held, glow;
float h(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float n(vec2 p) { vec2 i = floor(p), f = fract(p); f = f * f * (3. - 2. * f);
  return mix(mix(h(i), h(i + vec2(1, 0)), f.x), mix(h(i + vec2(0, 1)), h(i + vec2(1, 1)), f.x), f.y); }

const float RH = .37;              // the horizon
const float RIN = .5, ROUT = 1.08; // the disk
const float TILT = .15;            // how edge-on we see it

// The disk at radius r, angle a: fine grooves like a record, and gas drawn out
// into streaks along the orbit. The inner edge is sharp; inner parts turn faster.
float disk(float r, float a) {
  float k = (r - RIN) / (ROUT - RIN);
  if (k < 0. || k > 1.) return 0.;
  float edge = smoothstep(0., .025, k) * (1. - smoothstep(.6, 1., k));
  float s = a - t * 1.3 / pow(r, 1.5);
  vec2 o = vec2(cos(s), sin(s));
  float gas = n(vec2(r * 26., 0.) + o * 1.7) * .75 + n(vec2(r * 9., 3.) + o * 3.1) * .55;
  float grooves = .72 + .28 * sin(r * 64. + gas * 3.);
  return edge * pow(1. - k, 1.25) * grooves * (.3 + gas);
}

void main() {
  vec2 uv = (gl_FragCoord.xy - .5 * res) / (.5 * res.y);
  float R = length(uv), ang = atan(uv.y, uv.x);

  // Behind it all: black glass with a few stars, pulled into a ring by the hole.
  vec2 su = uv * (1. + RH * RH * 1.5 / max(R * R, .02));
  float c = t * .015; su = mat2(cos(c), -sin(c), sin(c), cos(c)) * su;
  vec2 cell = floor(su * 14.), f = fract(su * 14.) - .5;
  float star = step(.93, h(cell)) * smoothstep(.18, 0., length(f - (vec2(h(cell + 3.), h(cell + 7.)) - .5) * .5)) * .55;

  // The far side of the disk, bent over the top and under the bottom of the hole.
  float lr = (R - RH * 1.07) / (RH * .95);
  float arcs = 0.;
  if (lr > 0. && lr < 1.) {
    float side = uv.y > 0. ? 1. : .5;
    arcs = disk(RIN + lr * (ROUT - RIN) * .7, -ang) * pow(abs(sin(ang)), .8) * pow(1. - lr, 2.2) * 2.4 * side;
  }
  // The photon ring, and a fainter second one just outside it.
  float ring = exp(-pow((R - RH * 1.05) / .016, 2.)) * 1.6 + exp(-pow((R - RH * 1.17) / .01, 2.)) * .35;

  // The disk itself, nearly edge-on. The side coming at you is brighter.
  vec2 dp = vec2(uv.x, uv.y / TILT);
  float dr = length(dp);
  float d = disk(dr, atan(dp.y, dp.x)) * (1. - .62 * dp.x / max(dr, .01)) * 1.7;
  bool near = uv.y < 0.;

  float hole = 1. - smoothstep(RH - .012, RH + .006, R);
  float L = (star + arcs + ring + (near ? 0. : d)) * (1. - hole) + (near ? d : 0.);
  L *= .85 + .35 * glow;

  vec3 silver = vec3(.86, .9, 1.);
  vec3 hot = mix(vec3(1., .16, .1), vec3(1., .6, .42), clamp(L - .7, 0., 1.));
  vec3 amber = vec3(.98, .74, .22);
  vec3 tint = mix(mix(silver, hot, heat), amber, held);
  vec3 col = 1. - exp(-tint * L * 1.9);
  // The glass: near black, a touch lighter towards the top.
  col += vec3(.035, .037, .045) * (1. - hole) * smoothstep(-1., 1., uv.y);
  gl_FragColor = vec4(col, 1.);
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
    const d = Math.min(devicePixelRatio || 1, 3);
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
