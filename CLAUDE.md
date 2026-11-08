# Kakkoii

Open-source screen recorder. Tauri 2 (Rust core + web UI). Plan: `docs/PLAN.md`.

- Licence MIT. Only take code from MIT/Apache/BSD sources. Cap's app code is AGPLv3; only its `scap-*` and `cap-camera*` crates are MIT. Never copy Recordly or Screenity code.
- Light means light: solid metal surfaces in plain CSS (no screen grabs, no WebGL), `transform`/`opacity` animation only, nothing renders when idle.
- Commits: one-liners, no Co-Authored-By.
- Builds for Windows run on the Windows side (Rust + MSVC), not in WSL.
