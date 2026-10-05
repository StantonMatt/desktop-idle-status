# Desktop Idle Status

Purpose: identify desktop idle blockers in KDE Plasma. Components:
`kwin-bridge/` (C++ KWin native plugin), a future Rust service, and a future QML
plasmoid. Slice 1 implements only the bridge and nested test harness. Leave
`design/` untouched unless explicitly asked to change it.

## KWin safety

- Never load, install or enable experimental plugins in the user's real KWin
  session. No installation to `/usr` or `~/.local`, real `org.kde.KWin`
  `LoadPlugin` calls, real `kwinrc` edits, compositor restarts or signals.
- Test only in a nested/headless compositor on a new private D-Bus session.
  Use `--virtual`, offscreen Qt, and scoped HOME, XDG_CONFIG_HOME,
  XDG_RUNTIME_DIR, data/cache/state directories under
  `~/.cache/agent-scratch/desktop-idle-status/`. Remove desktop display and
  activation variables so no test dialogs can appear in the real session.
- Use `tests/kwin-bridge/run.sh`; do not invoke the integration Python script
  on the desktop session bus. Match the nested KWin owner PID before loading.
- Stop everything started by the task, including failure paths. Never stop
  user-owned applications, the real compositor, or another task's processes.
- All plugin work runs on KWin's main thread. Never retain raw Window pointers
  beyond a callback; use QPointer. No blocking I/O, file writes, threads,
  rendering/effects or input filters. The sole write action is ActivateWindow.
- Effective inhibitors come from KWin's exact input()->idleInhibitors() list;
  never infer membership from app names or visibility.
- Rebuild native plugins for each KWin release and verify the metadata IID.

## Workflow

Use SSH URLs for GitHub Git operations. Run the `review-fix-loop` skill before
committing or pushing. Worker agents leave changes uncommitted for the
orchestrator and do not start review loops themselves.

Small focused bridge builds and integration checks run directly. Full suites,
whole-repository type checks and production/release builds use
`/home/mjstanton/.local/bin/heavy`. Store task artifacts in the on-disk project
scratch directory, not RAM-backed `/tmp`.
