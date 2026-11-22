# NEON DE — Build Instructions

## 1. Prerequisites (Debian / Ubuntu)

Install the Rust toolchain (1.80.1+; verified on 1.93.1) and required Wayland/graphics development libraries:

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential \
  pkg-config \
  rustc \
  cargo \
  rustfmt \
  rust-clippy \
  libwayland-dev \
  libxkbcommon-dev \
  libudev-dev \
  libinput-dev \
  libseat-dev \
  libgbm-dev \
  libdrm-dev \
  libegl-dev \
  libgles-dev \
  libpixman-1-dev \
  libgtk-4-dev \
  libgtk4-layer-shell-dev \
  libadwaita-1-dev
```

## 2. Building NEON DE

```bash
# Debug build
cargo build --workspace

# Release build
cargo build --workspace --release

# Lint & formatting checks
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
```

## 3. Running in a Nested Session (Safe Development Mode)

Run NEON DE safely inside your existing Wayland or X11 desktop session:

```bash
# Launch nested compositor and auto-spawn a terminal inside NEON DE
./scripts/run-nested.sh

# Or invoke neon-compositor directly with a specific client
cargo run -p neon-compositor -- --command weston-terminal

# Launch without auto-spawning a client (prints WAYLAND_DISPLAY to connect clients manually)
cargo run -p neon-compositor
```

To connect additional Wayland clients manually to a running nested NEON DE instance (for example, if it binds `wayland-2`):

```bash
WAYLAND_DISPLAY=wayland-2 weston-simple-shm &
WAYLAND_DISPLAY=wayland-2 alacritty &
```

## 4. Software Rendering Fallback

If debugging GPU/driver issues or running without hardware acceleration:

```bash
LIBGL_ALWAYS_SOFTWARE=1 cargo run -p neon-compositor -- --command weston-terminal
```
