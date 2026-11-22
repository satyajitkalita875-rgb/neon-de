//! Display & rendering backends for **NEON DE**.
//!
//! Milestone 1 provides the nested `winit` + OpenGL ES (`GlesRenderer`) backend so that
//! development and verification run safely inside an existing Wayland or X11 host session.

pub mod winit;
