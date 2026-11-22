# NEON DE — Security Model

## 1. Threat Model & Core Principles

A Wayland compositor is a high-trust boundary handling input events, clipboard contents, window surfaces, and session state for every graphical application running under the user's UID.

NEON DE enforces:

1. **Wayland Client Isolation**: Clients can only access their own surfaces and explicitly granted protocol globals. No client can sniff another client's keystrokes or inspect arbitrary screen pixels.
2. **Protocol Validation & Robustness**: Malformed client requests or unexpected disconnects must terminate only the offending client connection (via Wayland protocol errors) and never panic or crash `neon-compositor`.
3. **Minimal Surface Exposure**: Never advertise a Wayland protocol global unless its handler is implemented and verified.
4. **Development Isolation**: Nested development runs inside an isolated `WAYLAND_DISPLAY` socket created in `$XDG_RUNTIME_DIR` and never modifies the host desktop session or login manager.

## 2. Planned Security Controls (Milestones 4–7)

- **IPC Authentication (M4)**: `SO_PEERCRED` UID verification and `0600` Unix socket permissions; strictly typed commands with zero arbitrary shell execution.
- **Session Lock (M5)**: `ext_session_lock_v1` preventing input or frame leakage to regular clients while locked.
- **Privilege Escalation (M7)**: Delegated strictly to `polkit`.
- **Screen Capture & Sharing (M7)**: Mediated exclusively via XDG Desktop Portal user consent dialogs.
- **Configuration Safety (M6)**: Bounds-checked deserialization with safe fallback defaults.

> **Verification Policy**: No security guarantee is claimed as enforced until backed by automated tests in the corresponding milestone.
