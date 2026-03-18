# RevyCraft

Small Bevy voxel sandbox with first-person movement, block placement/removal, strict Clippy checks, and a lightweight `HashMap`-backed world.

## Run

```bash
cargo run
```

## Controls

- `WASD`: move
- `Space`: jump
- `Mouse`: look
- `Left Click`: remove block
- `Right Click`: place selected block
- `1` / `2` / `3`: select `Grass` / `Dirt` / `Stone`
- `Esc`: release cursor
- `Left Click` after release: recapture cursor

## Lint And Tests

```bash
cargo lint
cargo test --all-targets
```

`cargo lint` is an alias for `cargo clippy --all-targets --all-features`, and the project enables strict Clippy groups directly in `Cargo.toml`.

## Terrain Presets

The startup terrain comes from `TerrainSettings::from_env()`. By default it uses `rolling_hills`.

To try a different preset for one run:

```bash
BEVY_TERRAIN_PRESET=plains cargo run
```

or:

```bash
BEVY_TERRAIN_PRESET=rugged cargo run
```

Available presets:

- `TerrainSettings::rolling_hills()`
- `TerrainSettings::plains()`
- `TerrainSettings::rugged()`
