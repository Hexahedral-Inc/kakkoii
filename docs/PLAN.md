# Kakkoii plan

Kakkoii (かっこいい) is Japanese for "cool". A light, open-source screen recorder that a 10-year-old can use. Bandicam power, toy-simple.

Status: phase 0 works (2026-11-07). Full-screen 4K60 H.264 recording on Windows, 3.3 MB app.

## Build on, don't fork

| Source | Licence | What we take |
|---|---|---|
| Cap `scap-*`, `cap-camera*` crates | MIT (the rest of Cap is AGPLv3, don't touch it) | Screen and webcam capture |
| `windows-capture` crate | MIT | Windows Graphics Capture |
| getopenscreen/openscreen | MIT | Port zoom, cursor and export logic, with credit |
| FFmpeg (LGPL build) | LGPL | Encoding with NVENC / QuickSync / VideoToolbox |
| bencho Dynamic Island, Liquid toggle, Glass bubble | MIT | Island and control motion |
| liquid-gooey | MIT | Mode chips melting into each other |
| Recordly (AGPL), Screenity (GPL), Kap (dead) | n/a | Ideas only, no code |

Kakkoii itself: MIT.

## Stack

Tauri 2: Rust core, web UI. Target is a ~10-15 MB install and under 3% CPU while recording, not counting the encoder.

## Modes

Screen · Screen + Face · Face only · Window · Area · GIF · Replay (last 30 s, always on while armed).

Three clicks from open to recording. No settings wall.

## Pieces

- **Island**: a black pill at the top centre. Red dot, timer, mic level. Tap it to grow into pause / stop / restart / draw / mute. It is hidden from its own recording (`WDA_EXCLUDEFROMCAPTURE` on Windows, `sharingType = .none` on macOS).
- **Face bubble**: circle, square or rounded. Drag it, snap it to corners, blur the background.
- **After screen**: trim, auto-zoom on clicks, export MP4/GIF, copy. One screen, no timeline maze.

## Glass that doesn't eat the CPU

- The OS draws the blur: Mica/Acrylic on Windows 11 and NSVisualEffectView on macOS, through the `window-vibrancy` crate. The compositor does it, so it costs us nothing.
- CSS `backdrop-filter` only on small surfaces, never full-window.
- Animate only `transform` and `opacity`. Springs come from Motion.
- Shaders (refraction, gooey) run only while something moves, then the canvas stops. Nothing loops when idle.
- No UI repaint while recording except the island timer (1 Hz) and the mic meter (rAF while the island is visible).
- Dopamine comes from feel, not noise: a spring overshoot on record, a soft tick sound, a satisfying "saved" morph of the island into a thumbnail.

## Phases

| # | Ships | Size |
|---|---|---|
| 0 | Tauri shell records the full screen to MP4 on Windows | 3-5 days |
| 1 | Modes, face bubble, island, mic + system audio. Windows first | 2-3 weeks |
| 2 | macOS (ScreenCaptureKit, signing, notarising) | 1-2 weeks |
| 3 | Trim, auto-zoom, cursor smoothing, GIF, replay | 2-3 weeks |
| 4 | Landing (kakkoii.app), GitHub release, Linux (PipeWire portal) | 1-2 weeks |

## Risks

- Windows builds need Rust + MSVC Build Tools on the Windows side. WSL can't capture the Windows desktop.
- macOS signing costs $99 a year. Unsigned Windows apps trigger SmartScreen.
- Wayland capture is painful, which is why Linux comes last.
- Webcam and screen audio drift. Timestamp everything off one clock.

## Names checked

kakkoii.app and getkakkoii.com were free on 2026-11-07. kakkoii.com and kakkoii.dev are taken.
