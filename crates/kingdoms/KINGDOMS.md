# Kingdoms (medieval towns)

This crate is the Synvox-fork feature code for generated medieval towns. It is **not** meant to be offered upstream. The world-gen hooks it uses *are* written so they can later be split into an upstream PR.

## What it generates

One town per 2048×2048 cell, a pure function of `(seed, cell)`. Each chunk writes only its own columns.

- **Castle** on high ground, at least 128×128 (8×8 chunks): contour-following polygonal curtain (not a rectangle), inner ward, mixed round / D / octagon / square towers with cone or crenellated tops, twin-round gatehouse, multi-mass keep, hall, chapel, stables, barracks, kitchen, courtyards, and a moat/ditch where the approach is low enough. Archetype (ridge, concentric, spur, coastal) is a function of seed and terrain. Walls step with the ridge. No flattening.
- **Town** on the downhill side: market on the flattest patch, well and fountain ring, gate plazas, timber-frame and cottage lots (WFC facades), church, tavern, blacksmith with a forge yard, longhouse, barn, windmill, farms outside the walls.
- **Hinterland** (same seed+cell plan, chunk-local writes): outlying hamlets, large farmsteads with crop rows / hay barns / windmills with a tower and diagonal-X cloth sails, orchards, fenced pastures, watchtowers on high ground, ruined forts, a monastery with cloister arcade, courtyard and bell tower, a dense graveyard (headstones, yews, lychgate), harbour/docks where the plot touches water, and a pillager palisade camp (tents in rows on lanes, a war tent, cages, a dark-oak outpost with an enclosed loft, trebuchet / covered ram / stacked ladders in an open siege yard, and a staked palisade with gaps and a wall-walk). One hero scene per town: a Gothic cathedral (cruciform nave / transepts / choir, twin west spires, flying buttresses, rose and lancet glass, steep deepslate-tile roof, furnished interior, cathedral close) on gentle ground, or a mountain fortress on steep relief. Terrain is not flattened.
- **Roads** by A* over sampled height with a ~1:4 slope cost, stairs when steeper, switchbacks when the path needs them. Neighbouring cells agree on a hashed crossing on the shared 2048-block edge and stitch outbound roads to that point from both sides. Overlay road columns outside hinterland occupancy keep the occupancy grid hinterland-sized. Bridges still form over water.
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
| `spawns` | Villager beds, workstations, wall/tower guard posts, market and keep rallies, gates, pillager camp tent spawn/rally, covered ram, siege ladders |

`spawns` is a pure function of seed+cell (planner height + occupancy). `y` is the sampled surface + 1, the standing height the stamper aims for, not a live chunk Y. The villager AI layer can query:

```rust
use pumpkin_kingdoms::{town_info, SpawnRole};

let info = town_info(seed, cell_x, cell_z)?;
let beds: Vec<_> = info.spawns.iter().filter(|s| s.role == SpawnRole::VillagerBed).collect();
let camp: Vec<_> = info.spawns.iter().filter(|s| s.role == SpawnRole::PillagerSpawn).collect();
```

Roles: `VillagerBed`, `Workstation`, `GuardPost`, `RallyMarket`, `RallyKeep`, `Gate`, `PillagerSpawn`, `PillagerRally`, `SiegeRam`, `SiegeLadder`.

`TownInfo` stays geometry only. Capture, siege jobs, and AI live in the director (`siege.rs`). Town ownership is `kingdoms-ownership.json` in the world folder and shows up on `battle_state.owner`.

Inter-cell roads meet at a crossing hashed from the unordered cell pair. The western/northern cell writes up to the column before the shared edge; the eastern/southern cell writes the edge column. Both sides use the same `z` (or `x`) so the road reads as one line.

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
  src/bin/raid_bot.rs  headless Java client that stands in a town so it ticks
```

`/locatetown` prints the nearest planned town, a sampled surface Y, and the castle size.

## Villagers, factions, and raids

This is the first combat step toward a 24/7 town war. Fork-only: no new AI scheduler in Pumpkin. Kingdoms adds goals on top of vanilla `GoalSelector`, `BowAttackGoal`, `BreakDoorGoal`, and pillager/vindicator raider goals. Guards use a kingdoms `ThrottledMeleeGoal` instead of vanilla `MeleeAttackGoal` so large fights do not path every 4 ticks.

### Factions

Each town belongs to **alder** or **raven** (even/odd cell). Villagers spawned in a town are its citizens. Pillagers are one hostile faction. Team relations:

- same kingdom (villagers, iron golems, players who `/kingdom join`) → allied, friendly fire cancelled
- kingdom vs pillagers → hostile
- two kingdoms → hostile (unused until capture)

Vanilla scoreboard teams `k_alder`, `k_raven`, `k_pillager` are created with friendly fire off so `is_allied_to` matches. Damage is also cancelled in `EntityDamageByEntity` because Pumpkin's living-damage path does not check teams.

`TownScene` / `TownInfoScene` reads [`TownInfo::spawns`](src/info.rs): guards at `Gate` (then `GuardPost`), archers on `GuardPost` (towers, wall walks, watchtowers), civilians on `VillagerBed`, rallies on `RallyKeep` / `RallyMarket`, and raids from `PillagerSpawn` tent columns in the hinterland camp (`PillagerRally` if a camp failed to place).

### Roles

| Role | Equipment | Alarm behaviour |
| --- | --- | --- |
| Guard | iron sword, chainmail | melee pillagers; rally to the threatened gate |
| Archer | bow, leather | hold a tower/wall; `BowAttackGoal` |
| Civilian | none | flee to the keep, ring a bell (`block.bell.use`), return after |
| Builder | none | stub: idle near the keep. Repair-by-diff (`plan.planned_block`) is later |

Iron golems keep vanilla melee / monster targeting and also rally.

### Raids

`/raid start <town> [size]` — town is `here` (cell containing the sender), quoted `"0,0"` / `"20261025:0:0"`, or an unquoted word like `here`. Commas and colons need quotes. Default size 24. Waves of pillagers (crossbow) and vindicators (melee + door break) spawn on `PillagerSpawn` tent columns in the hinterland camp, captains wear the ominous banner, and they march on the threatened gate. `/raid stop` ends every raid in the process.

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

On each director tick: spatial buckets (16 blocks) for target acquisition. Idle garrisons still `set_no_ai` stagger; a live raid does not, because skipping AI also skipped walking. Path-search cap 48/tick applies only to **squad leaders** refreshing live `navigator.set_progress`. Everyone else steers on a shared flow field (one BFS per gate/keep, 4-block cells, O(1) lookup). Followers keep cohesion with their leader; within ~16 blocks and LOS they walk straight at the target.

`ThrottledMeleeGoal` (kingdoms, not Pumpkin) path-refreshes on that same cadence instead of vanilla melee's ~4 tick interval.

Headless numbers (flow-field steering on a planned town, **not** a full server tick with physics/pathfinder). Release build (`CARGO_PROFILE_RELEASE_LTO=off`), seed `20261025`, 800 ticks. JSON: `/opt/cursor/artifacts/siege-bench.json`.

| N vs N | ms/tick | worst ms | melee engaged | peak melee | path capped |
| --- | ---: | ---: | ---: | ---: | ---: |
| 50 | 0.026 | 0.066 | 78 | 47 | 0/3102 |
| 100 | 0.057 | 0.139 | 156 | 93 | 0/6198 |
| 200 | 0.129 | 0.358 | 314 | 172 | 0/12506 |
| 400 | 0.319 | 0.916 | 586 | 249 | 5665/24514 |

200 vs 200 stays under ~10 ms/tick of sim AI, with most combatants reaching melee. Path budget is only spent on squad-leader refreshes; 400 vs 400 starts to clip those.

Live 200 vs 200 was 38–68 ms MSPT when vanilla `MeleeAttackGoal` / `RangedCrossbowAttackGoal` and `LivingEntity::push_entities` still ran. The director now tags raid soldiers `NoAI` and slides them on the flow field; batched target acquisition and throttled melee/bow replace the vanilla goals. The one upstream hook is skipping `push_entities` for NoAI mobs (documented next to the call in `living.rs`). Target: sustained ≤40 ms MSPT at 200 vs 200, ideally ≤25.

```bash
CARGO_PROFILE_RELEASE_LTO=off cargo run --release -p pumpkin-kingdoms --bin kingdoms-siege-bench -- --n 200 --ticks 800 --out-dir /opt/cursor/artifacts
```

Plots are ≥1024 px, wall/keep/gates under the dots, legend, start/mid/end frames plus a strip (`siege-{n}-start/mid/end/strip.png`).

### Live ticking (why a headless raid used to freeze)

Pumpkin ticks an entity only when its chunk is in `World::active_chunks` **and** `Level::is_chunk_loaded`. `forced_chunks` (what `/forceload` and the first raid code wrote) only fills `active_chunks`. The chunk scheduler still unpublishes block data unless a lasting `ChunkLoading::add_force_ticket` is held. `get_or_fetch_chunk` adds a FULL ticket then **removes** it after the fetch, so a one-shot load is not enough. Game tests already pair `forced_chunks` with `add_force_ticket`; vanilla `/forceload` in Pumpkin does not.

That is why a console-only raid spawned 40 raiders, civilians rang the bell in the ~1 s window while tickets were draining, then `raider_gate_dist` sat at 109.28 for thousands of ticks: the camp chunks were no longer `is_chunk_loaded`, so `World::tick` skipped them. `set_no_ai` was already cleared for an active raid.

The raid now leases those chunks with `add_force_ticket` (released on `/raid stop` / victory). The director also drives flow-field navigation every tick, and if a mob's `tick_count` is still 0 it slides along the field and applies melee itself so a missed ticket cannot freeze the fight.

A logged-in player is still how a real game ticks a region. `kingdoms-raid-bot` joins offline, stands at the threatened gate (client-authoritative `SPlayerPosition`), and answers keep-alives. Use it for live evidence:

```bash
# pumpkin.toml: networking.java.online_mode=false encryption=false (compression can stay on)
KINGDOMS_PLOT_DIR=/opt/cursor/artifacts cargo run --release -p pumpkin
# other terminal:
kingdoms-raid-bot --host 127.0.0.1:25565 --x GATE_X --y GATE_Y --z GATE_Z --seconds 180
# console:
raid start here 40
raid start here 200 200
```

`Server::get_tps()` is `1000 / get_mspt()`, not a capped 20 Hz counter. An empty tick at 0.5 ms reports ~2000 TPS. That number is inverted MSPT, not proof that entities are running, and not proof that they are not. Read `mspt` and the `n_ticking` line instead.

Live Pumpkin (release, LTO off, seed `20261025`, `kingdoms-raid-bot` standing at the threatened gate, `KINGDOMS_PLOT_DIR` plots). Combat is sampled `kingdoms combat` lines. Raid soldiers are `NoAI`; the director slides them on the flow field and applies throttled melee/bow. Vanilla goal A* and entity shoving are off (the shove skip is the one-line `push_entities` hook in `living.rs`).

| Fight | peak bodies | fight MSPT | director_ms | `Can't keep up` | notes |
| --- | ---: | ---: | ---: | --- | --- |
| before (vanilla goals) | ~300 | 38–68 ms | n/a | yes, during contact | PR #6 live 200 |
| 200 vs 200 (+ leftovers) | 197 / 355 | **9–20 ms** (peak 19.8) | 3–5 (133 at spawn) | start only (8.6 s / 172 ticks, chunk tickets) | ram breach; capture |
| 400 vs 400 (+ leftovers) | 400 / 708 | **10–30 ms** (peak 30.4 then 15) | 4–9 | none during fight | packed at gate |

Capture: `kingdoms town captured town=20261025:0:0 raiders_in_keep=27 hold=200`. Ownership file `world/kingdoms-ownership.json` → `"pillager"`. `battle_state.owner` exposes it. Gates closed at `(1504, 597)` and `(1327, 669)`; ram cleared 18 iron bars at the town gate.

Plots: `/opt/cursor/artifacts/live-r2.log`, `live-r2-strip.png`, `live-200-r2-strip.png`, `live-400-r2-strip.png`, `kingdoms-ownership.json`.

Gaps: 400 was stacked on leftover bodies so it is not a clean 400 vs 400; the start hitch is still force-load catch-up, not AI; archers at this scale deal director damage rather than arrow entities; the covered ram is a spawn/job, not a moving block.

### Commands

| Command | |
| --- | --- |
| `/locatetown` | nearest planned town |
| `/kingdom info` | town, kingdom, owner, battle counts, gate breach |
| `/kingdom join <kingdom>` | `alder` or `raven` (players fight with that side) |
| `/raid start <town> [size] [defenders]` | start a pillager raid. Optional `defenders` is how many kingdom combatants to field (extras spawn at the threatened gate). |
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

Renders. `--all` writes planner PNGs (`*-planner-*`) then the same views from real Features chunks cropped to the aerial overview (terrain, trees, the kingdoms decorator). Scene close-ups for camp, cathedral/fortress, mill, and all four aerials come from that Features fill:

```bash
cargo run -p pumpkin-kingdoms --bin kingdoms-pregen -- --all --out-dir kingdoms-renders
# faster layout check:
cargo run -p pumpkin-kingdoms --bin kingdoms-pregen -- --all --planner-only --out-dir kingdoms-renders
```
