# Kakkoii

A light, open-source screen recorder. Pick a mode, press record, done. Windows today; macOS and Linux are next.

- **Bar:** a command bar, bottom centre. Type a window name to record it. Alt+Shift+R opens it, or stops a recording.
- **Modes:** the screen, the screen with your face, just your face, or any window.
- **Island:** while recording, a small capsule at the top shows the time and a Stop button. It never shows up in the video.
- **3.3 MB app.** Hardware H.264 through Windows' own encoder.

## Let an agent drive it

Kakkoii listens on `127.0.0.1:4777` behind a token written to `%APPDATA%\Kakkoii\agent.json`.

- MCP (streamable HTTP): `http://127.0.0.1:4777/mcp`. The bar's "Copy the MCP command" row gives you the `claude mcp add` line.
- Tools: `status`, `list_targets`, `start_recording` (target: `screen`, `face`, `foreground`, or any window title or app name like `chrome`), `stop_recording`, `camera`, `list_recordings`, `compress` (needs ffmpeg).
- REST: `GET /v1/status`, `GET /v1/targets`, `GET /v1/recordings`, `POST /v1/record`, `POST /v1/stop`, `POST /v1/camera`, `POST /v1/compress`.

```bash
curl -H "Authorization: Bearer $TOKEN" -d '{"target":"chrome","max_seconds":30}' http://127.0.0.1:4777/v1/record
```

## Build

Rust + the MSVC build tools on Windows, then `cargo build --release` in `src-tauri`.

Plan: [docs/PLAN.md](docs/PLAN.md). MIT. Geist font (OFL) and Phosphor icons (MIT) are bundled under `ui/`.
