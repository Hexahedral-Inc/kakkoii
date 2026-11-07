//! The agent port. REST on /v1/* and MCP (streamable HTTP, JSON responses) on /mcp,
//! both on 127.0.0.1 behind a bearer token written to %APPDATA%\Kakkoii\agent.json.

use std::io::Read;
use std::sync::Arc;

use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::engine::{Engine, StartOpts};
use crate::recorder::Quality;

pub const PORT: u16 = 4777;

/// This run's token, for the "copy the MCP command" row.
pub static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn mcp_command() -> String {
    let t = TOKEN.get().cloned().unwrap_or_default();
    format!("claude mcp add --transport http kakkoii http://127.0.0.1:{PORT}/mcp --header \"Authorization: Bearer {t}\"")
}

fn token() -> String {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).expect("random");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Starts the server thread and writes the connection file agents read.
pub fn serve(engine: Arc<Engine>, config_dir: std::path::PathBuf) {
    let token = token();
    let _ = TOKEN.set(token.clone());
    let url = format!("http://127.0.0.1:{PORT}");
    let _ = std::fs::create_dir_all(&config_dir);
    let _ = std::fs::write(
        config_dir.join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "url": url,
            "mcp": format!("{url}/mcp"),
            "token": token,
            "header": format!("Authorization: Bearer {token}"),
        }))
        .unwrap(),
    );
    std::thread::spawn(move || {
        let Ok(server) = Server::http(("127.0.0.1", PORT)) else { return };
        for req in server.incoming_requests() {
            let engine = engine.clone();
            let token = token.clone();
            std::thread::spawn(move || handle(req, &engine, &token));
        }
    });
}

fn header(req: &Request, name: &'static str) -> Option<String> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string())
}

fn reply(req: Request, code: u16, body: Value) {
    let ct = Header::from_bytes("Content-Type", "application/json").unwrap();
    let _ = req.respond(Response::from_string(body.to_string()).with_status_code(code).with_header(ct));
}

fn handle(mut req: Request, engine: &Arc<Engine>, token: &str) {
    let path = req.url().split('?').next().unwrap_or("").to_string();
    if path == "/health" {
        return reply(req, 200, json!({ "ok": true, "app": "kakkoii", "version": env!("CARGO_PKG_VERSION") }));
    }
    let authed = header(&req, "Authorization").is_some_and(|v| v == format!("Bearer {token}"));
    if !authed {
        return reply(req, 401, json!({ "error": "missing or wrong bearer token; read %APPDATA%\\Kakkoii\\agent.json" }));
    }
    let mut raw = String::new();
    let _ = req.as_reader().take(1 << 20).read_to_string(&mut raw);
    let body: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
    let method = req.method().clone();

    if path == "/mcp" {
        return match method {
            Method::Post => mcp(req, engine, body),
            _ => reply(req, 405, json!({ "error": "POST JSON-RPC here" })),
        };
    }

    let result = match (method, path.as_str()) {
        (Method::Get, "/v1/status") => Ok(engine.status()),
        (Method::Get, "/v1/targets") => Ok(engine.targets()),
        (Method::Get, "/v1/recordings") => engine.recordings(),
        (Method::Post, "/v1/record") => call(engine, "start_recording", body),
        (Method::Post, "/v1/stop") => engine.stop(),
        (Method::Post, "/v1/pause") => engine.pause(true),
        (Method::Post, "/v1/resume") => engine.pause(false),
        (Method::Post, "/v1/camera") => call(engine, "camera", body),
        (Method::Post, "/v1/compress") => call(engine, "compress", body),
        (Method::Post, "/v1/trim") => call(engine, "trim", body),
        _ => return reply(req, 404, json!({ "error": "unknown route", "routes": ROUTES })),
    };
    match result {
        Ok(v) => reply(req, 200, v),
        Err(e) => reply(req, 400, json!({ "error": e })),
    }
}

const ROUTES: &[&str] = &[
    "GET /health", "GET /v1/status", "GET /v1/targets", "GET /v1/recordings",
    "POST /v1/record", "POST /v1/stop", "POST /v1/camera", "POST /v1/compress", "POST /v1/trim", "POST /mcp",
];

/// One dispatcher for REST and MCP tool calls.
fn call(engine: &Arc<Engine>, tool: &str, args: Value) -> Result<Value, String> {
    match tool {
        "status" => Ok(engine.status()),
        "list_targets" => Ok(engine.targets()),
        "list_recordings" => engine.recordings(),
        "start_recording" => {
            let opts: StartOpts = serde_json::from_value(normalise_target(args)).map_err(|e| e.to_string())?;
            engine.start(opts)
        }
        "pause_recording" => engine.pause(true),
        "resume_recording" => engine.pause(false),
        "stop_recording" => {
            crate::recorder::log("stop via agent API");
            engine.stop()
        }
        "camera" => engine.set_camera(args.get("on").and_then(Value::as_bool).unwrap_or(true)),
        "compress" => {
            let quality = serde_json::from_value(args.get("quality").cloned().unwrap_or(json!("small"))).unwrap_or(Quality::Small);
            engine.compress(
                args.get("file").and_then(Value::as_str).map(String::from),
                quality,
                args.get("gif").and_then(Value::as_bool).unwrap_or(false),
            )
        }
        "trim" => {
            let secs = |k| args.get(k).and_then(Value::as_f64);
            engine.trim(
                args.get("file").and_then(Value::as_str).map(String::from),
                secs("start").unwrap_or(0.0),
                secs("end").unwrap_or(f64::MAX),
            )
        }
        "open_trim" => engine.open_trim(args.get("file").and_then(Value::as_str).map(String::from)).map(|_| json!({ "opened": true })),
        _ => Err(format!("unknown tool {tool}")),
    }
}

/// Lets agents say `"target": "chrome"` or `"target": "screen"` instead of the tagged object.
fn normalise_target(mut args: Value) -> Value {
    if let Some(t) = args.get("target").and_then(Value::as_str).map(str::to_string) {
        args["target"] = match t.to_lowercase().as_str() {
            "screen" | "desktop" | "display" => json!({ "kind": "screen" }),
            "face" | "camera" | "webcam" | "me" => json!({ "kind": "face" }),
            "foreground" | "focused" | "active" => json!({ "kind": "foreground" }),
            _ => json!({ "kind": "window", "query": t }),
        };
    }
    let system = args.get("system_audio").and_then(Value::as_bool);
    let mic = args.get("mic").and_then(Value::as_bool);
    if system.is_some() || mic.is_some() {
        args["sound"] = json!({ "system": system.unwrap_or(true), "mic": mic.unwrap_or(false) });
    }
    if let Some(a) = args.get("area").cloned().filter(Value::is_object) {
        args["target"] = json!({ "kind": "area", "x": a["x"], "y": a["y"], "w": a["w"], "h": a["h"] });
    }
    if let Some(i) = args.get("monitor").and_then(Value::as_u64) {
        args["target"] = json!({ "kind": "monitor", "index": i });
    }
    args
}

fn tools() -> Value {
    json!([
        { "name": "status", "description": "Is Kakkoii recording? Returns target, file, seconds, camera, last file.",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "list_targets", "description": "Every display and open window you can record (title, process, size).",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "start_recording",
          "description": "Start recording. target: \"screen\" (default), \"foreground\", or any text matched against window titles and process names, e.g. \"chrome\", \"edge\", \"figma\". Saves an H.264 MP4 to the user's Videos\\Kakkoii folder.",
          "inputSchema": { "type": "object", "properties": {
              "target": { "type": "string", "description": "screen | face (just the webcam) | foreground | <window title or app name>" },
              "monitor": { "type": "integer", "description": "Record this display instead (1-based)." },
              "area": { "type": "object", "description": "Record only this box of the primary display, in physical pixels: {x, y, w, h}.",
                        "properties": { "x": { "type": "integer" }, "y": { "type": "integer" }, "w": { "type": "integer" }, "h": { "type": "integer" } } },
              "quality": { "type": "string", "enum": ["high", "balanced", "small"], "description": "small = 30 fps, light files" },
              "camera": { "type": "boolean", "description": "Show the face bubble on screen while recording." },
              "max_seconds": { "type": "integer", "description": "Stop on its own after this many seconds." },
              "system_audio": { "type": "boolean", "description": "Record what the computer plays (default true)." },
              "mic": { "type": "boolean", "description": "Record the microphone (default false)." },
              "name": { "type": "string", "description": "File name without extension." }
          } } },
        { "name": "pause_recording", "description": "Pause the recording. Nothing is recorded until resume_recording; the pause is cut out of the video.",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "resume_recording", "description": "Carry on after a pause.",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "stop_recording", "description": "Stop and save. Returns the file path, length and size.",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "camera", "description": "Show or hide the webcam face bubble.",
          "inputSchema": { "type": "object", "properties": { "on": { "type": "boolean" } }, "required": ["on"] } },
        { "name": "list_recordings", "description": "Saved recordings, newest first.",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "compress", "description": "Make a smaller copy of a recording (needs ffmpeg). Defaults to the last recording. gif: true makes a GIF.",
          "inputSchema": { "type": "object", "properties": {
              "file": { "type": "string" },
              "quality": { "type": "string", "enum": ["balanced", "small"] },
              "gif": { "type": "boolean" }
          } } },
        { "name": "trim", "description": "Cut a recording to start..end seconds, losslessly (no re-encode), into a new <name>-trim.mp4. Defaults to the last recording; leave out end to keep to the end.",
          "inputSchema": { "type": "object", "properties": {
              "file": { "type": "string" },
              "start": { "type": "number", "description": "seconds from the start" },
              "end": { "type": "number", "description": "seconds from the start" }
          } } },
        { "name": "open_trim", "description": "Open the trim window on screen so the user can cut a recording by hand. Defaults to the last recording.",
          "inputSchema": { "type": "object", "properties": { "file": { "type": "string" } } } }
    ])
}

fn mcp(req: Request, engine: &Arc<Engine>, msg: Value) {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    // Notifications get no body.
    if id.is_none() {
        let _ = req.respond(Response::empty(202));
        return;
    }
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": msg.pointer("/params/protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "kakkoii", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Kakkoii records the user's Windows screen, a window, or the screen with their face. Call list_targets to see what is open, start_recording, then stop_recording when asked."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            let args = msg.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            Ok(match call(engine, name, args) {
                Ok(v) => {
                    let mut r = json!({ "content": [{ "type": "text", "text": v.to_string() }], "isError": false });
                    if v.is_object() {
                        r["structuredContent"] = v;
                    }
                    r
                }
                Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
            })
        }
        _ => Err(json!({ "code": -32601, "message": format!("method not found: {method}") })),
    };
    let body = match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
    };
    reply(req, 200, body);
}
