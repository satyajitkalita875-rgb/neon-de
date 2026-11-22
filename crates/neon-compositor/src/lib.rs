//! Core Wayland compositor library for **NEON DE**.
//!
//! Structured along future workspace crate boundaries so that window management,
//! rendering, input handling, and backends can be extracted cleanly in later milestones.

pub mod backend;
pub mod grabs;
pub mod input;
pub mod state;
pub mod wayland;
pub mod window;
