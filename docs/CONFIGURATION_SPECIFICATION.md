# NEON DE — Configuration Specification

> **Status**: **Planned (Milestone 6)**, with isolated CLI/env configuration in Milestone 1.

## 1. Storage Location

User configuration will reside under `${XDG_CONFIG_HOME:-$HOME/.config}/neon-de/`:

- `config.toml` — versioned user configuration (`schema_version = 1`)
- `displays.toml` — persistent per-output monitor layout, scale, mode, and primary output
- `window-rules.toml` — user-defined matching rules

## 2. Safety & Validation Guarantees

- Strongly typed schema deserialization with per-field bounds validation.
- Invalid or unrecognized keys fall back to safe defaults with structured warnings; configuration errors must never crash `neon-compositor`.
- Atomic write-and-rename persistence, schema migration functions between versions, import/export, and reset-to-defaults support.
- Live hot-reload propagation to `neon-compositor` and `neon-shell` without session restart whenever feasible.
