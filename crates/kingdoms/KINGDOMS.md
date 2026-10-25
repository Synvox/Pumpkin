# Kingdoms (milestone 0)

This crate is the Synvox-fork feature code for generated medieval towns. It is **not** meant to be offered upstream. The world-gen hooks it uses *are* written so they can later be split into an upstream PR.

## Hooks (upstreamable)

These live in Pumpkin crates and do nothing when unused:

| Hook | Where | What it does |
| --- | --- | --- |
| `WorldGenExtensions` / `ChunkDecorator` | `pumpkin-world::generation::hooks` | Runs after vanilla feature/structure generation (`generate_features_and_structure` and the flat/custom Features step). Decorators see the center chunk plus the existing 1-chunk write neighborhood. |
| `StructureStartFilter` | same module, called from `ProtoChunk::set_structure_starts` | Can veto a vanilla structure start (for example villages inside a town plot). |
| `VanillaGenerator::sample_surface_height` | `pumpkin-world` generator + `Level` / `World` | Samples approximate terrain height at an arbitrary `x,z` without generating a chunk. Same noise-column sampler structures use. |
| `WorldPortalExt::on_chunk_populated` | `pumpkin-world::world`, fired when a chunk is upgraded to `Full` | The gameplay `World` turns this into the existing `ChunkPopulate` / `populate_chunk` event. Cheap no-op when no handlers are registered. |

Register per world:

```rust
world.register_chunk_decorator(decorator);
world.register_structure_start_filter(filter);
let y = world.sample_surface_height(x, z);
```

Native plugins can call those `World` / `Level` methods. The WASM plugin API still cannot reach vanilla world gen, which is why this crate is a native / statically linked plugin.

## Plugin layout

```
crates/kingdoms/
  KINGDOMS.md          this file
  src/lib.rs           install() + WorldInit attach
  src/town.rs          seed+cell planner (one town per 2048×2048)
  src/marker.rs        well decorator + village veto
  src/command.rs       /locatetown
  src/bin/pregen.rs    offline proof tool
```

Milestone 0 places **one cobblestone-and-oak well** at a deterministic cell center. Each chunk writes only the well blocks that fall inside it, so the result does not depend on chunk load order. The pad follows the local surface column (no area flattening).

`/locatetown` prints the nearest planned town and a sampled surface Y.

## Build and run

From the repo root:

```bash
cargo fmt --check
cargo clippy -p pumpkin-world -p pumpkin-core -p pumpkin-kingdoms -p pumpkin --all-targets --all-features
cargo nextest run -p pumpkin-world -p pumpkin-kingdoms --no-tests=pass
```

Headless server (writes config and world files into the working directory):

```bash
mkdir -p /tmp/pumpkin-kingdoms && cd /tmp/pumpkin-kingdoms
cargo run --manifest-path /path/to/Pumpkin/Cargo.toml -p pumpkin
# after "Started server":
locatetown
stop
```

The binary crate `pumpkin` calls `pumpkin_kingdoms::install` after plugins load, so you do not need to copy a `.so` into `./plugins`.

Offline proof (two generation orders + a top-down PNG):

```bash
cargo run -p pumpkin-kingdoms --bin kingdoms-pregen -- --seed 20261025 --cell 0,0 --out town.png
```

## How to rebase on upstream

1. Fetch Pumpkin-MC/Pumpkin `master` into this fork.
2. Rebase this branch (or merge) onto the new master.
3. The files most likely to conflict are the **small hook points** only:
   - `crates/pumpkin-world/src/generation/proto_chunk.rs` (`generate_features_and_structure`, `try_set_structure_start`)
   - `crates/pumpkin-world/src/chunk_system/generation_cache.rs` (Features / Full stages)
   - `crates/pumpkin-world/src/generation/generator/mod.rs` and `flat.rs` (extensions field)
   - `crates/pumpkin-world/src/world.rs` (`on_chunk_populated`)
   - `crates/pumpkin-world/src/level.rs` (register / sample helpers)
   - `crates/pumpkin-core/src/world/mod.rs` (`World` helpers + `WorldPortal`)
   - `crates/pumpkin/src/main.rs` (one `install` call)
4. `crates/kingdoms` should replay cleanly; it does not live upstream.
5. Keep hook behavior **zero-change when unused**. Do not reformat upstream files you did not mean to touch.

Later, the hook-only diff can be opened as an upstream PR without this crate.
