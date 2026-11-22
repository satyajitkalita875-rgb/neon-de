//! Display & rendering backends for **NEON DE**.
//!
//! - `winit`: Nested `winit` + OpenGL ES (`GlesRenderer`) backend for running safely inside
//!   an existing Wayland or X11 host session.
//! - `drm`: Bare-metal DRM/KMS + GBM + EGL/GLES + libseat + libinput + udev backend for
//!   standalone TTY / display-manager sessions.

pub mod drm;
pub mod winit;
