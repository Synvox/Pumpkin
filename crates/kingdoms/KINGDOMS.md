# Kingdoms (medieval towns)

This crate is the Synvox-fork feature code for generated medieval towns. It is **not** meant to be offered upstream. The world-gen hooks it uses *are* written so they can later be split into an upstream PR.

## What it generates

One town per 2048×2048 cell, a pure function of `(seed, cell)`. Each chunk writes only its own columns.

- **Castle** on high ground, at least 128×128 (8×8 chunks): contour-following polygonal curtain (not a rectangle), inner ward, mixed round / D / octagon / square towers with cone or crenellated tops, twin-round gatehouse, multi-mass keep, hall, chapel, stables, barracks, kitchen, courtyards, and a moat/ditch where the approach is low enough. Archetype (ridge, concentric, spur, coastal) is a function of seed and terrain. Walls step with the ridge. No flattening.
- **Town** on the downhill side: a cobbled market square with stalls, lantern posts, hedges and gardens, a town hall with a clocktower, well and fountain ring, gate plazas, biome-themed houses (plains timber-frame gables and 2–3 storey jettied overhangs on hill streets; taiga dark timber, steep snowy roofs and longhouses; desert sandstone, enclosed flat roofs with parapets, stepped domes, arcade/awning eaves and curved palms), church, tavern, blacksmith with a forge yard, barn, farms outside the walls. Cell `(0,0)` is the **royal capital**: a larger castle with a distinct inner and outer bailey and a palace keep.
- **Hinterland** (same seed+cell plan, chunk-local writes): outlying hamlets, large farmsteads with crop rows / hay barns, tower mills sited on the town edge or a hilltop with a reserved sail disc (radius 11, the full vertical plane) so all four oak-stock X-blades spin in open air, orchards, fenced pastures, watchtowers on high ground, ruined villages (several crumbling house shells around a cleared green and well), a monastery with cloister arcade, courtyard and bell tower, a dense graveyard (headstones, yews, lychgate), a harbour town (stone quay, cranes, fishing boats, piers, cog ships in water at least 3 deep or dredged in water only, warehouses, lighthouse) where the plot touches water, a jousting tournament (lists, stands, pavilions, trees cleared from the yard plus a 6-block margin) near the capital, a pillager palisade camp (tents in rows on lanes, a war tent, cages, a dark-oak outpost with an enclosed loft, trebuchet / covered ram / stacked ladders in an open siege yard, and a staked palisade with gaps and a wall-walk), and a hillside mine (timber adit portal into the hill, larger oak headframe with sheave wheel, cart track running out of the adit, spoil heaps, trees cleared from the yard plus a 6-block margin). A Gothic cathedral (cruciform nave / transepts / choir, twin west spires, flying buttresses, an 11-wide radial rose with spokes and petals, steep deepslate-tile roof, furnished interior, cathedral close) terraces into gentler slopes, and a mountain fortress on a peak, **both in the same cell when sites exist**. Terrain is not flattened beyond a building's own footprint.
- **Roads** by A* over sampled height with a ~1:4 slope cost, stairs when steeper, switchbacks when the path needs them. Neighbouring cells agree on a hashed crossing on the shared 2048-block edge and stitch outbound roads to that point from both sides. Overlay road columns outside hinterland occupancy keep the occupancy grid hinterland-sized. Bridges still form over water.
- **Palettes** by vanilla biome: oak/plains (timber-frame), spruce/taiga (dark timber, snow), sandstone/desert (Moorish/Levantine sandstone, still medieval). Castle walls mix stone bricks, cobble, mossy, cracked, andesite and tuff, with a darker cobble/andesite plinth.

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

A town flips only when attackers hold the keep courtyard **and** defenders there are wiped out or routed. The hold needs `max(8, 10% of the surviving raid)` attackers in the keep for 200 ticks (~10 s). Nearby players see a boss-bar and actionbar capture meter (`Capture 12/40 hold 80/200`). `/raid stop` and battle end (`Victory` / capture / retake) despawn leftover raid soldiers, extra defenders tagged `kingdoms:temp`, and ram display entities so a later fight does not stack bodies.

A moving battering ram is six block-display entities (dark-oak logs, oak slab roof, fence sides). Ram-job raiders push it along the road; nearby defenders chip its HP (80). It either breaches the iron-bar gate or is destroyed. Units rout when outnumbered 3:1 (at least 8 foes in 16 blocks) or their captain dies; a living captain rallies routed soldiers within 16 blocks.

When no HTTP director has talked to the API in the last 30 s, a war clock starts the next raid on a timer (`KINGDOMS_WAR_CLOCK`, default on; `KINGDOMS_WAR_CLOCK_TICKS`, default 2400 ≈ 2 min). It picks a cached town and the attacking side from current ownership. An authenticated HTTP call marks the external director as connected and pauses the clock.

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
| Civilian | none | work by day, home at night; flee to the keep and ring a bell (`block.bell.use`) on alarm |
| Builder | none | after a siege, walk to missing/wrong blocks vs `plan.planned_column` and place them (gates, then walls, then houses), one block / 10 ticks with a swing |

Iron golems keep vanilla melee / monster targeting and also rally.

### Raids

`/raid start <town> [size]` — town is `here` (cell containing the sender), quoted `"0,0"` / `"20261025:0:0"`, or an unquoted word like `here`. Commas and colons need quotes. Default size 24. Waves of pillagers (crossbow) and vindicators (melee + door break) spawn on `PillagerSpawn` tent columns in the hinterland camp, captains wear the ominous banner, and they march on the threatened gate. `/raid retake <town> [size]` sends kingdom soldiers to retake a pillager-owned town. `/raid stop` ends every raid in the process.

`/kingdom join <alder|raven>` is allowed for any player. It dyes leather armour in the team colour, puts the player on the scoreboard team (friendly fire off, name tags on), and sets respawn at that kingdom's keep. Membership is saved in `kingdoms-ownership.json` and reapplied on join.

Archers within ~32 blocks of a logged-in player shoot real arrow entities. Distant shots stay director damage so 400 vs 400 does not spawn hundreds of arrows.

Director API in-process:

```rust
pumpkin_kingdoms::start_raid(&world, &town_id, RaidSpec::new(40))?;
pumpkin_kingdoms::start_raid(&world, &town_id, RaidSpec::new(40).with_side(RaidSide::Kingdom))?;
pumpkin_kingdoms::set_target_town(raid_id, &other_town)?;
pumpkin_kingdoms::stop_raid(Some(&town_id));
let state = pumpkin_kingdoms::battle_state(&town_id);
```

### HTTP director (off by default)

For an external AI director (e.g. on Sails). Enable with env; bind is localhost so a Cloudflare Tunnel can expose it.

| Env | |
| --- | --- |
| `KINGDOMS_HTTP=1` | turn the server on (`KINGDOMS_DIRECTOR_HTTP` also works) |
| `KINGDOMS_HTTP_TOKEN` | bearer token (required, **≥ 32 characters**; shorter or missing refuses to bind). Alias `KINGDOMS_DIRECTOR_TOKEN`. Compared in constant time |
| `KINGDOMS_HTTP_BIND` | default `127.0.0.1:8742` |
| `KINGDOMS_HTTP_WEBHOOK` | optional URL; POSTs JSON events |
| `KINGDOMS_HTTP_WEBHOOK_SECRET` | HMAC-SHA256 key (defaults to the bearer token) |
| `KINGDOMS_WAR_CLOCK` | `0`/`false` disables the 24/7 loop (default on) |
| `KINGDOMS_WAR_CLOCK_TICKS` | ticks between auto-raids (default `2400`) |

Bodies over 16 KiB return 413. More than 20 requests/second return 429. A successful bearer auth pauses the war clock for 30 s.

Webhook body is JSON. Header `X-Kingdoms-Signature: sha256=<hex>` is HMAC-SHA256 of the raw body. Events: `raid_started`, `gate_breached`, `town_captured`, `player_joined_a_kingdom`.

```bash
export TOKEN=dev-token-must-be-32-chars-min
# pumpkin.toml world already running, with KINGDOMS_HTTP=1 KINGDOMS_HTTP_TOKEN=$TOKEN

curl -sS -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8742/state
curl -sS -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8742/war
curl -sS -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8742/towns/20261025:0:0
curl -sS -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"town":"here","size":40,"side":"pillager"}' http://127.0.0.1:8742/raids
curl -sS -H "Authorization: Bearer $TOKEN" -X POST http://127.0.0.1:8742/raids/1/stop
curl -sS -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"name":"Herald","message":"The gate is down"}' http://127.0.0.1:8742/chat
```

`GET /state` returns towns (owners, battles, troop counts) and logged-in player positions. `GET /war` is a front-line summary for the Sails war map (scores, clock, keep/gate, occupy hold). It is authenticated but does **not** pause the war clock. `POST /raids` also accepts `"side":"kingdom"` for a counter-raid and an optional `"camp"` (currently unused; waves still spawn from the hinterland camp / relief post).

`kingdoms-ownership.json` now also stores kingdom scores, `clock_tick`, `next_raid_id`, and `last_clock_tick` so a restart keeps the war. Town ownership and player membership were already there.

Civilians work at a workstation by day (ticks 2000–11000) and go home to a bed at night. The town bell still sends them to the keep when a raid starts. Guards patrol the wall between raids. Empty bed slots refill at most two citizens per 200 ticks, and never during a fight.

### Population

`ChunkPopulate` (PR #1) spawns a stable garrison: roughly 4–16 guards, 4–12 archers, 8–24 civilians, 4 builders, 2 golems, homes a function of the plan. Citizens are `PersistenceRequired` and tagged (`kingdoms:citizen`, `kingdoms:role:…`). Empty slots respawn if the home chunk is loaded. `/raid start 200` is 200 **raiders** against that garrison, not 200 extra villagers. Use the siege bench for N vs N.

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
| 400 vs 400 (fresh world) | **392 / 408** (800 ticking) | **14–19 ms** after contact (peak **31.87** at spawn) | 3–6 | start hitch only (8.7 s / 174 ticks) | ram 21 bars; capture; real arrows 25–52 near bots |

Capture: `kingdoms town captured town=20261025:0:0 raiders_in_keep=27 hold=200`. Ownership file `world/kingdoms-ownership.json` → `"pillager"`. `battle_state.owner` exposes it. Gates closed at `(1504, 597)` and `(1327, 669)`; ram cleared 18 iron bars at the town gate.

Plots: `/opt/cursor/artifacts/live-r2.log`, `live-r2-strip.png`, `live-200-r2-strip.png`, `live-400-r2-strip.png`, `kingdoms-ownership.json`.

Round 3 (fresh world, two `kingdoms-raid-bot`s on alder/raven, HTTP `POST /raids` size 400/400): peak **800** ticking bodies (`defenders=392 raiders=408`), fight MSPT **14–19** after the spawn hitch (peak **31.87** at tick 1500). One `Can't keep up` (8.7 s / 174 ticks) on chunk tickets, none during the fight. Real arrows: **25** at first contact, up to **52** while bots stood at the gate. Ram cleared 21 bars; capture `raiders_in_keep=4 hold=200`; ownership file has towns + player membership. Builders queued jobs and placed gate blocks before the raid. HTTP webhooks fired `player_joined_a_kingdom`, `raid_started`, `gate_breached`, `town_captured` with valid HMAC. A 40-soldier `side=kingdom` retake spawned after stop.

Plots: `/opt/cursor/artifacts/live-r3.log`, `live-r3-strip.png`, `live-r3-start-strip.png`, `live-r3-mid-strip.png`, `kingdoms-ownership-r3.json`, `webhook-r3.log`.

Round 4 (seed `20261025`, release, LTO off, two `kingdoms-raid-bot`s). Capture need is `max(8, 10% surviving)`; 4 raiders in an empty keep no longer flip a town. `/raid stop` despawned leftover soldiers (`despawned=36` then `29`). HTTP refused tokens shorter than 32 chars, compared in constant time, returned 413 over 16 KiB and 429 over 20 req/s. A six-part block-display ram walked the road (`x=1543→1506`) and breached 21 bars. 282 soldiers routed when a captain died. The war clock started a 24-man raid at tick 800 with no HTTP director connected.

| Fight | peak bodies | fight MSPT | director_ms | `Can't keep up` | notes |
| --- | ---: | ---: | ---: | --- | --- |
| war clock 24 | ~90 | 14–16 | 3–4 | start 8.5 s / 169 ticks | clock raid; ram spawned+breached 21; rout 71 `captain_down`; stop despawned 36 |
| 200 vs 200 | **401** ticking (200/201) | **14–24** (peak **24.21** at contact, then 14–17) | 3–7 | start 8.6 s / 171 ticks | ram walked camp→gate with 60 crew; rout 282 `captain_down=true`; arrows 64–112 |
| 80 vs 20 | ~99 | **9–14** | 3 | spawn hitch 107 ms | ram `1543,482 → 1506,591` then **breached 21 bars**; `keep_r=4 keep_d=0` for ~200 ticks, **no capture** (need=8) |

Plots: `/opt/cursor/artifacts/live-r4.log`, `live-r4-200-strip.png`, `live-r4-80-strip.png`, `live-r4-clock-strip.png`, `kingdoms-ownership-r4.json`.

Gaps: the start hitch is still force-load; a full keep capture (`need` attackers in the courtyard) did not complete live (4 in keep is now correctly not enough); leather kits are server-side (bots do not render); the war clock only sees towns already in the director cache (a player, bot, or `/raid start`).

Round 5 (seed `20261025`, release, LTO off, two `kingdoms-raid-bot`s at the gate/keep). Raid spawn is pre-warmed (~80 ticks of force tickets at 12/tick) then soldiers spawn 8 per tick. `kingdoms-ownership.json` stores scores and the war clock. `GET /war` is the Sails front-line map (auth, does not pause the clock). Civilians work by day / home at night / keep on alarm; guards patrol the wall; beds refill two citizens per 200 ticks when the town is quiet.

| Fight | peak bodies | fight MSPT | director_ms | `Can't keep up` | notes |
| --- | ---: | ---: | ---: | --- | --- |
| r4 80 vs 20 (before) | ~99 | 9–14 | spawn **107.72** | start 8.5–9.7 s / 169–193 ticks | `keep_r=4`, **no capture** (need=8) |
| r5 120 vs 40 (after) | **186** ticking (67/122 at first contact) | **9.7–15.6** (worst fight tick **15.56** at spawn complete, then 10–13) | **2.9–5.0** | join only 8.4 s / 168 ticks (player chunk load). **No raid-spawn hitch ≥ 50 ms** | ram `1542,486 → 1507,589` then **breached 21**; `keep_r=8 keep_d=0 hold=200` **captured**; webhook `town_captured`; ownership `"pillager"` score 1; war clock Kingdom counter-raid |

Capture: `kingdoms town captured town=20261025:0:0 raiders_in_keep=8 need=8 hold=200 score=pillager`. Restart reloaded `tick`, `scores.pillager=1`, `owner=pillager`, `last_clock_tick` via `GET /war`.

Plots: `/opt/cursor/artifacts/live-r5.log`, `live-r5-strip.png` (4460×1024), `live-r5-clock-strip.png`, `kingdoms-ownership-r5.json`, `webhook-r5.log`, `war-r5-persist.json`.

Gaps: player join still force-loads ~170 chunks in one tick (~8.5 s); the war clock fired on the same tick as the flip so builders did not get a quiet repair window in that run (the clock now waits out the 400-tick hold lockout); leftover camp hostiles can keep `alarm` true after the raid, which also blocks repair and bed respawn.

### Commands

| Command | |
| --- | --- |
| `/locatetown` | nearest planned town |
| `/kingdom info` | town, kingdom, owner, battle counts, gate breach |
| `/kingdom join <kingdom>` | `alder` or `raven`. Leather team colour, name tag, friendly fire off, respawn at the keep |
| `/raid start <town> [size] [defenders]` | start a pillager raid. Optional `defenders` is how many kingdom combatants to field (extras spawn at the threatened gate). |
| `/raid retake <town> [size]` | kingdom counter-raid to retake a pillager-owned town |
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
