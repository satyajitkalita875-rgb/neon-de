# Contributing to NEON DE

## Engineering Standards

1. **Verify Smithay APIs against source**: Never guess Smithay signatures; inspect the pinned crate source and examples before implementing protocol handlers.
2. **Incremental Milestones**: Keep changes focused on the current milestone. Do not introduce empty stub crates or fake UI controls.
3. **Formatting & Linting**: All PRs/commits must pass:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets
   cargo test --workspace
   ```
4. **Honest Documentation**: Update `docs/WAYLAND_PROTOCOLS.md` and `docs/DEVELOPMENT_ROADMAP.md` whenever protocols or features move from planned to implemented.
5. **Host Safety**: Never modify host login managers or replace the host Wayland compositor during development; test in nested mode or isolated VMs.
