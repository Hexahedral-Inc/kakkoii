<p align="center"><img src="docs/logo/kakkoii.svg" width="96" alt="Kakkoii" /></p>

<h1 align="center">Kakkoii</h1>

A light, open-source screen recorder. Pick a mode, press record, done. Windows today; macOS and Linux are next.

- **Bar:** a command bar, bottom centre. Type a window name to record it. Alt+Shift+R opens it, or stops a recording.
- **Modes:** the screen, an area you drag out, the screen with your face, just your face, or any window.
- **Pause:** from the island or Alt+Shift+P. The pause is cut out of the video.
- **Orb:** a small black lens with the record dot floats over your screen and never shows in a recording. Click it (or **Alt+Shift+K**) and its menu comes out as smaller lenses orbiting it: Screen, Area, A window, With face, Just face, Screenshot, Face bubble, Recordings, Settings. Drag it anywhere. While recording, it shows the time; click for Stop and Pause.
- **Screenshot:** a PNG of the whole display in Pictures\Kakkoii, without Kakkoii's own windows.
- **Island:** while recording, a small capsule at the top shows the time and a Stop button. It never shows up in the video.
- **Sound:** what the computer plays, and your mic. Face modes turn the mic on.
- **Face box:** drag it anywhere, scroll to resize, double-click for a circle. It shows in the video.
- **Trim:** after a recording, press Trim, drag the two handles, save. It cuts without re-encoding, to the exact frame, and keeps the original.
- **Small app.** Hardware H.264 + AAC through Windows' own encoder.

## Let an agent drive it

Kakkoii listens on `127.0.0.1:4777` behind a token written to `%APPDATA%\Kakkoii\agent.json`.

- MCP (streamable HTTP): `http://127.0.0.1:4777/mcp`. The bar's "Copy the MCP command" row gives you the `claude mcp add` line.
- Tools: `status`, `list_targets`, `start_recording` (target: `screen`, `face`, `foreground`, or any window title or app name like `chrome`; or `area: {x, y, w, h}` in pixels), `pause_recording`, `resume_recording`, `stop_recording`, `camera`, `list_recordings`, `trim` (file, start, end in seconds; lossless), `screenshot`, `open_trim` (puts the trim window in front of you), `compress` (needs ffmpeg).
- REST: `GET /v1/status`, `GET /v1/targets`, `GET /v1/recordings`, `POST /v1/record`, `POST /v1/stop`, `POST /v1/pause`, `POST /v1/resume`, `POST /v1/camera`, `POST /v1/compress`, `POST /v1/trim`.

```bash
curl -H "Authorization: Bearer $TOKEN" -d '{"target":"chrome","max_seconds":30}' http://127.0.0.1:4777/v1/record
```

## Build

Rust + the MSVC build tools on Windows, then `cargo build --release` in `src-tauri`.

Plan: [docs/PLAN.md](docs/PLAN.md). `src-tauri/vendor/windows-capture` is a copy of [windows-capture](https://github.com/NiiightmareXD/windows-capture) (MIT) with two additions to the encoder: region crop and pause offsets. MIT. Geist font (OFL) and Phosphor icons (MIT) are bundled under `ui/`.
