//! Core Wayland compositor library for **NEON DE**.
//!
//! Structured along workspace crate boundaries (`neon-workspaces`, `neon-window-manager`,
//! `neon-effects`, `neon-scene`, `neon-renderer`, `neon-ipc`).

pub mod backend;
pub mod grabs;
pub mod input;
pub mod ipc;
pub mod state;
pub mod wayland;
pub mod window;
