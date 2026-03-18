# RevyCraft

Small Bevy voxel sandbox with first-person movement, block placement/removal, strict Clippy checks, chunk streaming terrain generation, caves, biome variation, and chunk autosave.

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

## World Generation

Terrain is generated per chunk from a deterministic seed with:

- biome variation (`plains`, `hills`, `dry_stone`)
- layered surface blocks
- 3D cave carving with a protected surface buffer
- automatic chunk load/unload around the player
- autosave for edited chunks

The current world is saved under `worlds/seed-<seed>/`.

## Environment

`TerrainSettings::from_env()` controls the terrain preset. By default it uses `rolling_hills`.

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

You can also change the seed and visible chunk radius:

```bash
BEVY_WORLD_SEED=12345 BEVY_VIEW_RADIUS=3 cargo run
```

- `BEVY_WORLD_SEED`: deterministic world seed, defaults to a fixed built-in value
- `BEVY_VIEW_RADIUS`: chunk load radius around the player, defaults to `2`
