# NEON DE — Security Model

## 1. Threat Model & Core Principles

A Wayland compositor is a high-trust boundary handling input events, clipboard contents, window surfaces, and session state for every graphical application running under the user's UID.

NEON DE enforces:

1. **Wayland Client Isolation**: Clients can only access their own surfaces and explicitly granted protocol globals. No client can sniff another client's keystrokes or inspect arbitrary screen pixels.
2. **Protocol Validation & Robustness**: Malformed client requests or unexpected disconnects must terminate only the offending client connection (via Wayland protocol errors) and never panic or crash `neon-compositor`.
3. **Minimal Surface Exposure**: Never advertise a Wayland protocol global unless its handler is implemented and verified.
4. **Development Isolation**: Nested development runs inside an isolated `WAYLAND_DISPLAY` socket created in `$XDG_RUNTIME_DIR` and never modifies the host desktop session or login manager.
5. **Authenticated IPC (Implemented & Tested — Milestone 4)**: The NEON DE IPC socket resides in a `0700` runtime directory with `0600` socket permissions, enforces same-UID peer credentials via Linux `SO_PEERCRED`, bounds frame sizes to 256 KiB, and exposes strictly typed desktop actions with zero arbitrary shell execution.
6. **Session Lock Isolation (Implemented & Tested — Milestone 5)**: `ext_session_lock_v1` clears regular client keyboard/pointer focus upon locking, blocks compositor shortcuts while locked, and renders exclusively the lock surface over an opaque security curtain (`#05070D`) so zero desktop windows or shell overlays leak.

## 2. Planned Security Controls (Milestones 6–7)

- **Privilege Escalation (M7)**: Delegated strictly to `polkit` and PAM authentication inside `neon-lockscreen`.
- **Screen Capture & Sharing (M7)**: Mediated exclusively via XDG Desktop Portal user consent dialogs.
- **Configuration Safety (M6)**: Bounds-checked deserialization with safe fallback defaults.

> **Verification Policy**: No security guarantee is claimed as enforced until backed by automated tests in the corresponding milestone.
