# Kingdoms (medieval towns)

This crate is the Synvox-fork feature code for generated medieval towns. It is **not** meant to be offered upstream. The world-gen hooks it uses *are* written so they can later be split into an upstream PR.

## What it generates

One town per 2048×2048 cell, a pure function of `(seed, cell)`. Each chunk writes only its own columns.

- **Castle** on high ground, at least 128×128 (8×8 chunks): contour-following polygonal curtain (not a rectangle), inner ward, mixed round / D / octagon / square towers with cone or crenellated tops, twin-round gatehouse, multi-mass keep, hall, chapel, stables, barracks, kitchen, courtyards, and a moat/ditch where the approach is low enough. Archetype (ridge, concentric, spur, coastal) is a function of seed and terrain. Walls step with the ridge. No flattening.
- **Town** on the downhill side: market on the flattest patch, well and fountain ring, gate plazas, timber-frame and cottage lots (WFC facades), church, tavern, blacksmith with a forge yard, longhouse, barn, windmill, farms outside the walls.
- **Hinterland** (same seed+cell plan, chunk-local writes): outlying hamlets, large farmsteads with crop rows / hay barns / windmills, orchards, fenced pastures, watchtowers on high ground, ruined forts, a monastery, a graveyard, harbour/docks where the plot touches water, and a pillager palisade camp with tents and siege engines placed away from the town. Terrain is not flattened.
- **Roads** by A* over sampled height with a ~1:4 slope cost, stairs when steeper, switchbacks when the path needs them. Outbound roads aim at neighbouring cells and become bridges over water.
- **Palettes** by vanilla biome: oak/plains, spruce/taiga, sandstone/desert. Castle walls mix stone bricks, cobble, mossy, cracked, andesite and tuff, with a darker cobble/andesite plinth.

Siting uses `VanillaGenerator::sample_surface_height` (the same noise column structures use). Placement uses the real chunk surface and only local foundations, terraces, stilts, or retaining walls. Lots, streets, and farms stay on dry land; leftover water becomes harbor, dock, bridge, or moat. The town rectangle shrinks off wet edges instead of walling in a lake. Courtyards are grass with paths and trees. Houses are timber-frame with cobble foundations, plaster, **closed gables** (triangular wall infill to the roof), chimneys, and furnished rooms (bed, table, chest, furnace, lantern). Castle keeps and halls sit on one ridge floor so they do not smear into dirt terraces. Castles mix stone variants, merlons, wall walks, a portcullis, and banners. Roof stairs sit on walls: perimeter columns rise to `roof_y - 1`, cone/hip/spire caps are filled, and doors are real door blocks rather than air.

Every castle building has storey floors, a ladder between floors, lanterns/candles (spacing aimed at light ≥ 8), and room furniture. A flood-fill check (`watertight::check_town`) starts from interior air and must not escape except through panes, doors, trapdoors, fences, or bars, which count as closed. Roof cells must have a solid block under them. Interior floor cells must not see the sky (a missing roof fails even when the flood stays on the same XZ).

### Interiors (inspiration, not copies)

Room layout follows how medieval Minecraft builds are usually furnished, not a schematic dump: BdoubleO100-style layered keep rooms and lighting; Grian-style timber halls with stair/slab tables, wool tapestries and hanging lanterns; the usual Mumbo-style tutorial kit (function first: bed, storage, work blocks); Forge Labs 100-days castles (great hall next to kitchen, chapel as its own mass); generic “medieval interior design” guides; and real-castle rooms (great hall with dais, solar, bedchamber, chapel, barracks, kitchen, cellar). Armor stands are skipped (entities). World gen writes **block states only** — chests, furnaces, beds and lecterns do not get block entities from `ChunkDecorationContext::set_block_state`.

## Query API (for later gameplay)

The planner is geometry only today, but each town exposes a serializable [`TownInfo`](src/info.rs) so a later 24/7 war (two AI kingdoms, capture, siege, bot soldiers) can ask “where is the keep / gate / market?” without re-deriving the plan.

```rust
use pumpkin_kingdoms::info::{town_info, town_info_at, TownInfo};

let info = town_info(seed, cell_x, cell_z)?;          // cached cell
let info = town_info_at(seed, block_x, block_z)?;     // cell containing a column
let json = info.to_json()?;
```

`TownInfo` is a pure snapshot of the deterministic plan:

| Field | Use later |
| --- | --- |
| `id` (`seed:cell_x:cell_z`) | Stable town identity |
| `cell`, `center`, `bounds` | Which region a force owns |
| `palette` | Biome materials |
| `capture_point` / `market` / `well` | Capture / rally |
| `town_wall`, `gates`, `towers` | Siege pathing and wall walks |
| `castle` (keep, gatehouse, barbican, inner bailey, halls, barracks…) | Garrison and breach targets |
| `roads` (polylines + endpoints) | March graph |
| `buildings` | Civilian / economic sites |
| `scenes` | Hinterland and civic scenes (farmstead, hamlet, camp, monastery, …) |
| `spawns` | Villager beds, workstations, wall/tower guard posts, market and keep rallies, gates, pillager camp spawn/rally |

`spawns` is a pure function of seed+cell (planner height + occupancy). `y` is the sampled surface + 1, the standing height the stamper aims for, not a live chunk Y. The villager AI layer can query:

```rust
use pumpkin_kingdoms::{town_info, SpawnRole};

let info = town_info(seed, cell_x, cell_z)?;
let beds: Vec<_> = info.spawns.iter().filter(|s| s.role == SpawnRole::VillagerBed).collect();
let camp: Vec<_> = info.spawns.iter().filter(|s| s.role == SpawnRole::PillagerSpawn).collect();
```

Roles: `VillagerBed`, `Workstation`, `GuardPost`, `RallyMarket`, `RallyKeep`, `Gate`, `PillagerSpawn`, `PillagerRally`.

No capture, siege, or AI logic lives here yet. `TownPlan` still holds the occupancy grid used to stamp chunks; `TownInfo` is the small struct game systems should store.

Inter-cell roads are truncated to this cell's occupancy. They point at the neighbouring town but do not stitch pixel-perfect at the 2048-block cell border.

### Planned blocks (for later repair)

After a siege, bots can restore a town by diffing the live world against the **intended** blocks of the plan. That query is a deterministic function of seed+cell (noise height samples, then the same stamper world gen uses). It does **not** regenerate chunks.

```rust
// From seed + cell, or any column in that cell. Needs a registered generator
// (the running overworld) or a plan already in cache.
let plan = pumpkin_kingdoms::plan_from_cache(seed, cell_x, cell_z)?;
let plan = pumpkin_kingdoms::plan_at(seed, x, z)?;

let column = plan.planned_column(x, z);          // all intended Y in one column
let block = plan.planned_block(x, y, z);         // one cell, or None
let block = plan.planned_at(BlockPos::new(x, y, z));
let keep = plan.planned_keep_blocks();           // castle keep volume
let house = plan.planned_building_blocks(lot_id); // iterate one building
let region = plan.planned_blocks_in(bounds);

// Same lookups without holding the plan:
let block = pumpkin_kingdoms::planned_block_at(seed, x, y, z);
let column = pumpkin_kingdoms::planned_column_at(seed, x, z);
let house = pumpkin_kingdoms::planned_building_blocks_for(seed, cell_x, cell_z, lot_id);
let region = pumpkin_kingdoms::planned_blocks_in_at(seed, bounds);
```

Each [`PlannedBlock`](src/stamp.rs) is `{x,y,z,state}` where `state` is the vanilla `BlockStateId` as `u16`. Last write in the stamper wins, same as world gen.

These Y values come from the planner height sampler, not from a finished chunk. Live placement still follows the real column surface, so a repaired column can sit 1–3 blocks off the in-world floor. That is enough to rebuild walls and buildings; a later pass can snap to the live surface if needed.

No siege-repair bots live here yet. This is the lookup they will call.

## Hooks (upstreamable)

These live in Pumpkin crates and do nothing when unused:

| Hook | Where | What it does |
| --- | --- | --- |
| `WorldGenExtensions` / `ChunkDecorator` | `pumpkin-world::generation::hooks` | Runs after vanilla feature/structure generation. |
| `StructureStartFilter` | same module | Vetoes vanilla villages inside a town plot. |
| `VanillaGenerator::sample_surface_height` | generator + `Level` / `World` | Samples terrain height without generating a chunk. |

## Layout

```
crates/kingdoms/
  KINGDOMS.md
  src/lib.rs           install() + WorldInit attach
  src/plan.rs          city plan (cached, seed+cell)
  src/scenes.rs        hinterland scenes + extra lots
  src/castle.rs        parametric castle
  src/buildings.rs     lots + WFC facades
  src/info.rs          TownInfo + spawn points
  src/stamp.rs         per-column placement
  src/interior.rs      furniture (same-column, deterministic)
  src/watertight.rs    flood-fill envelope check
  src/render.rs        top-down + isometric + cutaway PNG
  src/bin/pregen.rs    offline proof + renders + watertight report
```

`/locatetown` prints the nearest planned town, a sampled surface Y, and the castle size.

## Villagers, factions, and raids

This is the first combat step toward a 24/7 town war. Fork-only: no new AI scheduler in Pumpkin. Kingdoms adds goals on top of vanilla `GoalSelector`, `MeleeAttackGoal`, `BowAttackGoal`, `BreakDoorGoal`, and pillager/vindicator raider goals.

### Factions

Each town belongs to **alder** or **raven** (even/odd cell). Villagers spawned in a town are its citizens. Pillagers are one hostile faction. Team relations:

- same kingdom (villagers, iron golems, players who `/kingdom join`) → allied, friendly fire cancelled
- kingdom vs pillagers → hostile
- two kingdoms → hostile (unused until capture)

Vanilla scoreboard teams `k_alder`, `k_raven`, `k_pillager` are created with friendly fire off so `is_allied_to` matches. Damage is also cancelled in `EntityDamageByEntity` because Pumpkin's living-damage path does not check teams.

Scene posts (beds, workstations, camps) are **not** on `TownInfo` yet. `TownScene` / `TownInfoScene` derives guard posts from gates + barracks, archer posts from towers + wall samples, civilian homes from lots, raid camps from the outer road endpoint. Swap the trait when the parallel scene-metadata work lands.

### Roles

| Role | Equipment | Alarm behaviour |
| --- | --- | --- |
| Guard | iron sword, chainmail | melee pillagers; rally to the threatened gate |
| Archer | bow, leather | hold a tower/wall; `BowAttackGoal` |
| Civilian | none | flee to the keep, ring a bell (`block.bell.use`), return after |
| Builder | none | stub: idle near the keep. Repair-by-diff (`plan.planned_block`) is later |

Iron golems keep vanilla melee / monster targeting and also rally.

### Raids

`/raid start <town> [size]` — town is `here`, `cell_x,cell_z`, or `seed:cx:cz`. Default size 24. Waves of pillagers (crossbow) and vindicators (melee + door break) spawn at a camp outside the walls, captains wear the ominous banner, and they march on the threatened gate. `/raid stop` ends every raid in the process.

Director API (HTTP later, not in this crate):

```rust
pumpkin_kingdoms::start_raid(&world, &town_id, RaidSpec::new(40))?;
pumpkin_kingdoms::set_target_town(raid_id, &other_town)?;
pumpkin_kingdoms::stop_raid(Some(&town_id));
let state = pumpkin_kingdoms::battle_state(&town_id);
```

### Population

`ChunkPopulate` (PR #1) spawns a stable garrison: roughly 4–16 guards, 4–12 archers, 8–24 civilians, 1 builder, 2 golems, homes a function of the plan. Citizens are `PersistenceRequired` and tagged (`kingdoms:citizen`, `kingdoms:role:…`). Empty slots respawn if the home chunk is loaded. `/raid start 200` is 200 **raiders** against that garrison, not 200 extra villagers. Use the siege bench for N vs N.

### Scale / LOD

On each director tick: spatial buckets (16 blocks) for target acquisition, `set_no_ai` stagger (combat every tick, alert every 2, idle every 8), path-search cap 48/tick. Vanilla `MeleeAttackGoal` still pathfinds on ticks it runs; we cannot throttle that without changing Pumpkin.

Headless numbers (goal + steering on a planned town, **not** a full server tick with physics/pathfinder). Debug build, seed `20261025`, 400 ticks:

| N vs N | ms/tick | worst ms | defender kills | raider kills | path capped |
| --- | --- | --- | --- | --- | --- |
| 50 | 0.44 | 0.57 | 9 | 1 | 2.2k / 21k |
| 100 | 0.92 | 1.19 | 1 | 0 | 21k / 40k |
| 200 | 2.14 | 3.34 | 3 | 0 | 81k / 100k |

At 200 vs 200 the 48 path-searches/tick cap rejects most steering, so few units close to melee in 400 ticks. 50 vs 50 still scores kills. These numbers are the director/LOD/bucket loop plus a 0.25-block step; vanilla `Navigator` + entity physics on a live world will be slower.

```bash
cargo run -p pumpkin-kingdoms --bin kingdoms-siege-bench -- --n 200 --ticks 400 --out-dir kingdoms-siege
```

### Commands

| Command | |
| --- | --- |
| `/locatetown` | nearest planned town |
| `/kingdom info` | town, kingdom, battle counts |
| `/kingdom join <kingdom>` | `alder` or `raven` (players fight with that side) |
| `/raid start <town> [size]` | start a pillager raid |
| `/raid stop` | stop all kingdoms raids |

## Build and run

```bash
cargo fmt --check
cargo clippy -p pumpkin-kingdoms --all-targets --all-features
cargo nextest run -p pumpkin-kingdoms --no-tests=pass
```

Headless server:

```bash
mkdir -p /tmp/pumpkin-kingdoms && cd /tmp/pumpkin-kingdoms
cargo run --manifest-path /path/to/Pumpkin/Cargo.toml -p pumpkin
# after "Started server":
locatetown
stop
```

Renders. `--all` writes planner PNGs (`*-planner-*`) then the same views from real Features chunks (terrain, trees, the kingdoms decorator):

```bash
cargo run -p pumpkin-kingdoms --bin kingdoms-pregen -- --all --out-dir kingdoms-renders
# faster layout check:
cargo run -p pumpkin-kingdoms --bin kingdoms-pregen -- --all --planner-only --out-dir kingdoms-renders
```
