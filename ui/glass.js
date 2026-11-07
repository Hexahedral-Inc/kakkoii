/**
 * Liquid glass, ported 1:1 from Sage Desktop (ui/LiquidGlass.tsx). The desktop
 * behind the window shows through as soft colour, the rim bends like the lip of
 * a lens with a hint of colour split, light comes from the top left, and the
 * corners are drawn by the shader so nothing square sits behind the curve.
 * The Rust side (glass_frame) hands over the patch of desktop behind us.
 *
 * Cheap on purpose: it looks behind once on show, after a move, then every
 * `every` ms; it draws one frame per look, never a render loop.
 */
const VERT = `attribute vec2 p; varying vec2 uv; void main(){ uv = vec2(p.x * .5 + .5, .5 - p.y * .5); gl_Position = vec4(p, 0., 1.); }`;
const FRAG = `precision mediump float;
varying vec2 uv;
uniform sampler2D tex;
uniform vec2 size, offset, win;
uniform float radius, bezel, tint, has;
float sd(vec2 c){ vec2 b = size * .5; vec2 q = abs(c) - b + radius; return length(max(q, 0.)) + min(max(q.x, q.y), 0.) - radius; }
vec3 tap(vec2 px){ return texture2D(tex, clamp((px + offset) / win, vec2(.001), vec2(.999))).rgb; }
vec3 soft(vec2 px, float r){
  vec3 s = tap(px) * .2;
  s += (tap(px + vec2(r, 0.)) + tap(px - vec2(r, 0.)) + tap(px + vec2(0., r)) + tap(px - vec2(0., r))) * .125;
  s += (tap(px + vec2(r, r)) + tap(px - vec2(r, r)) + tap(px + vec2(r, -r)) + tap(px - vec2(r, -r))) * .075;
  return s;
}
void main(){
  vec2 px = uv * size, c = px - size * .5;
  float d = sd(c);
  if (d > .75) { gl_FragColor = vec4(0.); return; }
  float aa = clamp(.75 - d, 0., 1.);
  float inset = -d;
  vec2 e = vec2(.75, 0.);
  vec2 n = normalize(vec2(sd(c + e.xy) - sd(c - e.xy), sd(c + e.yx) - sd(c - e.yx)) + 1e-5);
  float k = clamp(1. - inset / bezel, 0., 1.);
  float bend = k * k * (3. - 2. * k) * k;
  vec3 col;
  if (has > .5) {
    vec2 base = px - n * bend * bezel * .85;
    float r = 1.5 + bend * 3.;
    col.g = soft(base, r).g;
    col.r = soft(base - n * bend * 2.6, r).r;
    col.b = soft(base + n * bend * 2.6, r).b;
    float l = dot(col, vec3(.299, .587, .114));
    col = mix(vec3(l), col, 1.18);
    col = mix(col, vec3(0.), tint);
  } else {
    col = vec3(.04, .04, .055);
  }
  vec2 L = normalize(vec2(-.55, -.85));
  float lip = smoothstep(2.6, 0., inset);
  float face = max(dot(n, L), 0.);
  col += vec3(1.) * (lip * (.16 + .62 * pow(face, 1.6)) + .07 * bend * face);
  col *= 1. - lip * .28 * max(dot(n, -L), 0.);
  col += vec3(1.) * .025 * (1. - uv.y);
  gl_FragColor = vec4(col * aa, aa);
}`;

/** How dark the glass gets: just enough for white words over what's behind it. */
function tintFor(bmp) {
  const c = document.createElement('canvas');
  c.width = c.height = 12;
  const x = c.getContext('2d', { willReadFrequently: true });
  x.drawImage(bmp, 0, 0, 12, 12);
  const d = x.getImageData(0, 0, 12, 12).data;
  let sum = 0, hi = 0;
  for (let i = 0; i < d.length; i += 4) { const l = (0.299 * d[i] + 0.587 * d[i + 1] + 0.114 * d[i + 2]) / 255; sum += l; hi = Math.max(hi, l); }
  const mean = sum / (d.length / 4);
  const t = Math.min(1, Math.max(0, (mean * 0.75 + hi * 0.25 - 0.18) / 0.62));
  return 0.18 + 0.42 * t * t * (3 - 2 * t);
}

export function liquidGlass(host, { radius = 14, bezel = 18, every = 1500 } = {}) {
  const { invoke } = window.__TAURI__.core;
  const win = window.__TAURI__.window.getCurrentWindow();
  const cv = document.createElement('canvas');
  cv.className = 'lg';
  cv.setAttribute('aria-hidden', 'true');
  Object.assign(cv.style, { position: 'absolute', inset: '0', width: '100%', height: '100%', zIndex: '0', pointerEvents: 'none' });
  host.prepend(cv);

  const gl = cv.getContext('webgl', { premultipliedAlpha: true, alpha: true, antialias: false });
  if (!gl) return () => {};
  const sh = (type, src) => { const s = gl.createShader(type); gl.shaderSource(s, src); gl.compileShader(s); return s; };
  const prog = gl.createProgram();
  gl.attachShader(prog, sh(gl.VERTEX_SHADER, VERT));
  gl.attachShader(prog, sh(gl.FRAGMENT_SHADER, FRAG));
  gl.linkProgram(prog);
  gl.useProgram(prog);
  gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog, 'p');
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  const tex = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, tex);
  for (const [k, v] of [[gl.TEXTURE_MIN_FILTER, gl.LINEAR], [gl.TEXTURE_MAG_FILTER, gl.LINEAR], [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE], [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE]]) gl.texParameteri(gl.TEXTURE_2D, k, v);
  const U = (n) => gl.getUniformLocation(prog, n);
  let has = 0, tint = 0.32, alive = true;

  const draw = () => {
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const w = cv.clientWidth, h = cv.clientHeight;
    if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(h * dpr)) { cv.width = Math.round(w * dpr); cv.height = Math.round(h * dpr); }
    gl.viewport(0, 0, cv.width, cv.height);
    gl.uniform2f(U('size'), w, h);
    const r = cv.getBoundingClientRect();
    gl.uniform2f(U('offset'), r.left, r.top);
    gl.uniform2f(U('win'), window.innerWidth, window.innerHeight);
    gl.uniform1f(U('radius'), radius);
    gl.uniform1f(U('bezel'), bezel);
    gl.uniform1f(U('tint'), tint);
    gl.uniform1f(U('has'), has);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  };

  // Blurred once on the CPU side, small: a smooth blur costs nothing this way.
  const soft = document.createElement('canvas');
  const take = (bmp) => {
    if (!alive) return;
    const w = Math.max(8, Math.ceil(bmp.width / 3)), h = Math.max(8, Math.ceil(bmp.height / 3)), pad = 10;
    soft.width = w; soft.height = h;
    const sx = soft.getContext('2d');
    sx.filter = 'blur(4px) saturate(1.15)';
    sx.drawImage(bmp, -pad, -pad, w + pad * 2, h + pad * 2);
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, soft);
    tint = tintFor(bmp);
    has = 1;
    bmp.close();
    draw();
  };

  let busy = false, timer = 0, moved = 0;
  const look = async () => {
    if (busy || !alive || document.hidden) return;
    busy = true;
    try {
      if (await win.isVisible()) {
        const bytes = await invoke('glass_frame', { label: win.label });
        take(await createImageBitmap(new Blob([bytes], { type: 'image/jpeg' })));
      }
    } catch { /* keep the last frame */ }
    busy = false;
  };
  const loop = () => { void look(); timer = setTimeout(loop, every); };
  loop();
  const soon = () => { clearTimeout(moved); moved = setTimeout(() => void look(), 140); };
  const offs = [win.onMoved(soon), win.onResized(soon), win.onFocusChanged(soon)];
  const ro = new ResizeObserver(draw);
  ro.observe(cv);
  draw();
  return () => {
    alive = false;
    clearTimeout(timer);
    ro.disconnect();
    for (const off of offs) void off.then((f) => f());
    gl.getExtension('WEBGL_lose_context')?.loseContext();
  };
}
