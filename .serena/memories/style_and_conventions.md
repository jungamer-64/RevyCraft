# Style and conventions
- Language: Rust 2024 edition workspace.
- Lints: workspace enforces `warnings = deny` and Clippy `all`, `pedantic`, `cargo`, and `nursery` as deny.
- Formatting: standard `cargo fmt` formatting.
- Architectural convention: `revy-voxel-semantic` is the canonical owner for shared semantic DTO/contracts; `revy-voxel-core` is engine-internal; `mc-plugin-host` should access gameplay reads through `revy-server-gameplay-bridge` rather than depending on `revy-voxel-core` directly.
- Docs are a first-class source of truth; contributor-facing architecture decisions are documented in `docs/contributors/runtime-and-plugin-architecture.md`.
- The repo intentionally distinguishes build artifacts from packaged runtime artifacts: runtime reads `runtime/plugins/`, not `target/` directly.