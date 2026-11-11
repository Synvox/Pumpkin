# Kingdoms (medieval towns)

This crate is the Synvox-fork feature code for generated medieval towns. It is **not** meant to be offered upstream. The world-gen hooks it uses *are* written so they can later be split into an upstream PR.

## What it generates

One town per 2048×2048 cell, a pure function of `(seed, cell)`. Each chunk writes only its own columns.

- **Castle** on high ground, at least 128×128 (8×8 chunks): contour-following polygonal curtain (not a rectangle), inner ward, mixed round / D / octagon / square towers with spire, cone, hip or crenellated tops and banners, twin-round gatehouse, multi-mass keep with a keep spire, hall, chapel, stables, barracks, kitchen, courtyards, and a moat/ditch where the approach is low enough. Archetype (ridge, concentric, spur, coastal) is a function of seed and terrain. Walls step with the ridge. No flattening.
- **Town** on the downhill side: a packed cobbled market square (10+ stalls with awnings, barrels, crates and hay, a well/fountain, a town hall with clocktower facing the square, and tight 2–3 storey jettied frontage on all four sides), lantern posts, hedges and gardens, gate plazas, biome-themed houses (plains timber-frame gables and 2–3 storey jettied overhangs on hill streets; taiga dark timber, steep snowy roofs and longhouses; desert sandstone, enclosed flat roofs with parapets, stepped domes, arcade/awning eaves and curved palms), church, tavern, blacksmith with a forge yard, barn, cottages / workshops / inns packed along the streets where the slope is gentle, farms outside the walls. Cell `(0,0)` is the **royal capital**: a larger castle with a distinct inner and outer bailey and a palace keep, plus a two-ring lower city (inner civic wall and a 3-wide crenellated outer curtain that follows the ground, with a 2–3 block cobble pomerium inside, a dry fosse outside, round interval towers taller than the townhouse roofs about every 24 blocks, and gatehouses where roads enter). The same two-ring treatment (curtain, pomerium, fosse or a wet sea wall on the coast, round towers, in-town cathedral, extramural suburb) applies to other megacities: taiga in spruce and cobble, desert in sandstone, and harbour in stone with a sea wall. The lower city packs 2–3 storey jettied townhouses wall-to-wall along the lanes, with mixed plaster colours, some stone ground floors and a few tile-red roofs, plus back gardens and small squares, so the aerial reads as a walled city rather than grass between districts. Streets carry market stalls, well heads, hanging signs, lamp posts, carts and hay. The in-town cathedral follows the biome: plains and harbour keep a Gothic church (seed-varied west towers, crossing and spire height, flying buttresses); desert is a sandstone basilica with a minaret-style bell tower (balcony ring) and a hemispherical gold/terracotta dome (13–15 wide, drum with windows, lantern); taiga is a spruce-and-stone Romanesque church with round towers and steep shingle roofs. Cottages, a tannery and an almshouse straggle outside the main gate. Districts split the capital into a market quarter around the fountain square, a craftsmen's row of smithies and tanneries, a noble quarter of townhouses and gardens on the castle side, and a docks quarter at the downhill gate; 1-wide lanes weave the lower city. A paved inner-bailey court sits between keep and hall so a keep flag plinth has a pad.
- **Hinterland** (same seed+cell plan, chunk-local writes): outlying hamlets, large farmsteads with crop rows / hay barns, tower mills sited on the town edge or a hilltop with a reserved sail disc (radius 11, the full vertical plane) so all four oak-stock X-blades spin in open air, extra ridge mills, orchards, woodland copses, fenced sheep pastures, a lumber camp with stacked timber, extra crop fields on leftover grass, watchtowers on high ground, ruined villages (several crumbling house shells around a cleared green and well), a hilltop priory with cloister garth, chapter house, scriptorium, herb garden, nave, bell tower and a two-wide gravel switchback with cobble retaining walls and stairs at the turns, a dense graveyard (headstones, yews, lychgate), a harbour town (stone-and-timber quay at least 8 deep on pilings down to the seabed, several 3×3 cranes with booms, a fish market of stalls on the waterfront inside the sea wall, breakwater with a tapered octagonal lighthouse — banding, gallery, glowstone lantern behind glass — on the seaward tip clear of the cog, warehouses, a harbourmaster house, barrels and crates, fishing boats, piers, and a cog with a 15–19-long hull tapering to a pointed bow over 4+ rows, a rising sheer, dark-oak and spruce planks, forecastle and sterncastle, a mast at least 12 high with a yard, a crow's nest, a slightly billowed white-wool sail (offset middle column), a small jib, fence/chain rigging to the rails, a lantern on the prow, and a flag at the masthead; fishing boats are small hulled skiffs with a mast and scrap of sail) only on real coast or a lake at least 40×40 with depth ≥ 3, a second walled village on a west island or peninsula when a dry patch has water on two or more sides, a riverside town with a stone bridge, cobble ford, fishing weirs, watermills (a closed mill house on the bank, a standing vertical water wheel at least 9 across, standing out from the wall by a gap with a visible axle, a dark-oak rim against paler birch spokes and paddles half-submerged; a sluice and a mill race; lily pads are stripped from flowing river water) and docks, suburbs and outskirts along the roads (farm hamlets, orchards, windmills, roadside cottages, workshops and inns, wayside shrines, hedgerows, field walls), a cliffside or river-gorge town (a carved ravine or river, houses jettied over the drop, retaining walls with lanterns, stairs, and a fortified bridge spanning the gap), a walled hill-fort manor with a gatehouse and furnished hall, a walled abbey vineyard village on a hillside (vine rows on contour-following terraces outside the cloister wall, with retaining walls, posts and trellis, cloister, winepress house, cellar, and cottages on the uphill terrace), a taiga stave church with dark-oak stave walls, stepped gables and dragon-head peaks, sited clear of the cathedral so calcite spires do not clip into the yard, a desert caravanserai (walled courtyard, arcade, minarets and lodging), a jousting tournament (sand lists with a tilt barrier, two-to-three-row wooden stands, 4–6 striped peaked pavilions, a raised roofed royal box in the kingdom's colours, and gates with weapon racks) sited beside the castle, a palisaded siege camp outside the castle (2–3 A-frame trebuchets facing the keep, tent rows, a command tent, cook fire, smithy, and a ditch with an earth bank), a walled hinterland market (staked palisade, guildhall, stalls and a well), a mill hamlet on a stream bank (enclosed mill house with a pitched roof, a standing 5–7 block water wheel over a channelled race, millstones and sacks inside, a granary on staddle stones and cottages), hilltop beacons (stone brazier towers) chained along hinterland roads, a pillager palisade camp (tents in rows on lanes, a war tent, cages, a dark-oak outpost with an enclosed loft, trebuchet / covered ram / stacked ladders in an open siege yard, and a staked palisade with gaps and a wall-walk), a hillside mine (timber adit portal into the hill, larger oak headframe with sheave wheel, cart track running out of the adit, spoil heaps), an open quarry cut as a stepped pit of 3–5 stone benches with a ramp, two timber jib cranes, a cart track, stacked ashlar and a mason's lodge at the rim, scattered 100-days-style points of interest between towns (a roofless Tintern-style ruined abbey with broken gables, empty pointed tracery, a partial arcade, moss and cobble rubble, ivy, a collapsed tower stump and a graveyard; a bandit hideout in a ravine with a staked palisade around a spread camp (open grass between the palisade, tents and fire), 3–4 separate wool tents, a campfire with a cooking spit, loot piles of chests and barrels, a stolen wagon, and lookouts on stilts; a hermit's round tower with a door, balcony, chimney and herb garden; a stone circle of tall menhirs with a fallen lintel; a castle ruin on a hill with a broken curtain, keep stump and corner towers; a roadside gallows at a crossroads; a toll bridge with a furnished gatehouse over a river; a charcoal-burners' clearing with a hut, coal mounds and hearths; and a roadside inn with a two-storey jettied tavern (dormers, interior bar, kegs, tables, hearth and rooms), a cobbled coaching yard with a well, parked wagon, hitching posts, stacked hay by the stable, and a hanging sign on a bracket over the door), and a siege-scarred **frontier town** on the Alder/Raven border (damaged stone curtain, collapsed towers, rubble, scorch marks, siege trenches, burned houses, trebuchet pits). Feature trees and floating leaves/lily pads are stripped from the whole town plan (walled town plus a 16-block stamp margin covering curtain, pomerium and fosse, market crop, a keepout around roofed lots, and every scene footprint plus a 6-block margin). Planned orchard and garden trees stay; leftover Features canopies taller than those saplings are still aired out. A cathedral (plains/harbour Gothic with a rose, calcite tracery and flying buttresses; desert basilica with a gold dome and minaret; taiga Romanesque with round towers and steep spruce roofs; furnished interior and a close with a graveyard, cloister arcade and bishop's house) terraces into gentler slopes, and a mountain fortress on a peak, **both in the same cell when sites exist**. Shoreline lots stand on stone footings or pilings down to solid ground, not on one-block pillars over water. Terrain is not flattened beyond a building's own footprint.
- **Roads** by A* over sampled height with a ~1:4 slope cost, stairs when steeper, switchbacks when the path needs them. Neighbouring cells agree on a hashed crossing on the shared 2048-block edge and stitch outbound roads to that point from both sides. Overlay road columns outside hinterland occupancy keep the occupancy grid hinterland-sized. Roads prefer dry ground; water crossings become 3–5-wide arched stone bridges with piers every 6–8 blocks, parapets, causeway approaches, and a small gate tower on a long span. Hinterland inns, walled markets, mill hamlets, beacons, quarries, monasteries, toll bridges, ruined abbeys and castle ruins connect to the city gates with terrain-following dirt and gravel tracks (path blocks, worn edges, occasional cobble milestones and wayside shrines).
- **Palettes** by vanilla biome: oak/plains (timber-frame), spruce/taiga (dark timber, snow), sandstone/desert (Moorish/Levantine sandstone, still medieval). Castle walls mix stone bricks, cobble, mossy, cracked, andesite and tuff, with a darker cobble/andesite plinth.

Siting uses `VanillaGenerator::sample_surface_height` (the same noise column structures use). Placement uses the real chunk surface and only local foundations, terraces, stilts, or retaining walls. Lots, streets, and farms stay on dry land; leftover water becomes harbor, dock, bridge, or moat. The town rectangle shrinks off wet edges instead of walling in a lake. Courtyards are grass with paths and trees. Houses are timber-frame with cobble foundations, plaster, **closed gables** (triangular wall infill to the roof), chimneys, and furnished rooms (bed, table, chest, furnace, lantern). Castle keeps and halls sit on one ridge floor so they do not smear into dirt terraces. Castles mix stone variants, merlons, wall walks, a portcullis, and banners. Roof stairs sit on walls: perimeter columns rise to `roof_y - 1`, cone/hip/spire and gable ridges are capped with full blocks so keep and hall roofs stay enclosed, a square keep spire uses equal Chebyshev faces (stairs on every ring, a short apex finial) so the near faces match the far ones, keep masses stamp even when hall/kitchen/chapel/tower occupancy later overwrites the keep cell, and doors are real door blocks rather than air.

Every castle building has storey floors, a ladder between floors, lanterns/candles (spacing aimed at light ≥ 8), and room furniture. A flood-fill check (`watertight::check_town`) starts from interior air and must not escape except through panes, doors, trapdoors, fences, or bars, which count as closed. Roof cells must have a solid block under them. Interior floor cells must not see the sky (a missing roof fails even when the flood stays on the same XZ). `watertight::check_floating_fragments` flags any connected component of placed blocks that is not 6-connected to terrain, water, or a grounded parent structure.

### Interiors (inspiration, not copies)

Room layout follows how medieval Minecraft builds are usually furnished, not a schematic dump: BdoubleO100-style layered keep rooms and lighting; Grian-style timber halls with stair/slab tables, wool tapestries and hanging lanterns; the usual Mumbo-style tutorial kit (function first: bed, storage, work blocks); Forge Labs 100-days castles (great hall next to kitchen, chapel as its own mass); generic “medieval interior design” guides; and real-castle rooms (great hall with dais, solar, bedchamber, chapel, barracks, kitchen, cellar). Armor stands are skipped (entities). World gen writes **block states only** — chests, furnaces, beds and lecterns do not get block entities from `ChunkDecorationContext::set_block_state`.

## Query API (for later gameplay)

The planner is geometry only today, but each town exposes a serializable [`TownInfo`](src/info.rs) so a later 24/7 war (two AI kingdoms, capture, siege, bot soldiers) can ask “where is the keep / gate / market?” without re-deriving the plan.

```rust
use pumpkin_kingdoms::info::{town_info, town_info_at, TownInfo};

let info = town_info(seed, cell_x, cell_z)?;          // memory/disk only; never generates
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

Roles: `VillagerBed`, `Workstation`, `GuardPost`, `RallyMarket`, `RallyKeep`, `Flag`, `Gate`, `PillagerSpawn`, `PillagerRally`, `SiegeRam`, `SiegeLadder`, `SiegeTreb`.

`TownInfo` stays geometry only. Capture, siege jobs, and AI live in the director (`siege.rs`). Town ownership is `kingdoms-ownership.json` in the world folder and shows up on `battle_state.owner`.

Each castle keeps a flag plinth in the inner bailey (never the gatehouse): a 3×3 stone pedestal, oak-fence mast, owner banner, and a ring of stone pressure plates. A town flips when attackers (players of the attacking kingdom, or the raid's bot standard-bearer) stand on that plate with **no defenders within 4 blocks** for 200 ticks (~10 s, `KINGDOMS_HOLD_TICKS`). The hold needs `max(3, 5% of the surviving raid)` on the plate. A defender on the plate or within 4 blocks **pauses** the clock; an empty plate **decays** progress. Hiding in a wall or cellar does nothing. Defenders reclaim the same way. Nearby players see a boss-bar (`Flag 3/3 hold 80/200`); captains shout on start/pause/flip. `/raid stop` and battle end (`Victory` / capture / retake) despawn leftover raid soldiers, extra defenders tagged `kingdoms:temp`, and ram display entities so a later fight does not stack bodies. The plate, mast, and banner are indestructible to players, trebuchet punches, and repair.

A moving battering ram is six block-display entities (dark-oak logs, oak slab roof, fence sides). Ram-job raiders push it along the road; nearby defenders chip its HP (80). It either breaches the iron-bar gate or is destroyed. Units rout when outnumbered 3:1 (at least 8 foes in 16 blocks) or their captain dies; a living captain rallies routed soldiers within 16 blocks.

When no HTTP director has talked to the API in the last 30 s, a war clock starts the next raid on a timer (`KINGDOMS_WAR_CLOCK`, default on; `KINGDOMS_WAR_CLOCK_TICKS`, default 2400 ≈ 2 min). After a flip it waits out the 400-tick hold lockout **plus** a 400-tick repair window so builders can work for the new owner. It can run several concurrent fronts (`KINGDOMS_WAR_CLOCK_FRONTS`, default 1) and alternates pillager raids with alder↔raven border marches. Size/extras: `KINGDOMS_WAR_CLOCK_SIZE` (default 24) and `KINGDOMS_WAR_CLOCK_DEFENDERS`. An authenticated HTTP call that is not observational (`GET /war`, `GET|POST /lines`, `GET|POST /heroes`) marks the external director as connected and pauses the clock.

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

Capture sets a per-town [`DamageState`](src/damage.rs) (`intact` → `scarred`, then `ruined` on a repeat capture). `planned_block` reads that map and returns the damaged variant (curtain holes, cobble rubble, coal scorch) until repair builders step it back (`ruined` → `scarred` → `intact`).

```rust
use pumpkin_kingdoms::{DamageState, damage_state, set_damage_state};

set_damage_state(&info.id, DamageState::Scarred);
assert_eq!(damage_state(&info.id), DamageState::Scarred);
```

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
  src/damage.rs        capture/repair damage_state for planned_block
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
| `KINGDOMS_BODY_BUDGET` | live (materialised) soldiers the war clock respects (default `240`) |
| `KINGDOMS_PROFILE=1` | log director/front/body/ticket/RSS every 100 ticks |

Bodies over 16 KiB return 413. More than 20 requests/second return 429. A successful bearer auth pauses the war clock for 30 s.

Webhook body is JSON. Header `X-Kingdoms-Signature: sha256=<hex>` is HMAC-SHA256 of the raw body. The full request/response contract is below so a Sails war-map can be built against it.

### Sails quickstart

Pumpkin binds the director on localhost. Sails reaches it through a Cloudflare Tunnel. Observational `GET /war` does not pause the in-game war clock; `POST /raids` and other mutating calls do for 30 s.

**Token.** Generate at least 32 characters and put the same value on both sides:

```bash
export TOKEN="$(openssl rand -hex 24)"   # 48 chars
# pumpkin.toml world:
KINGDOMS_HTTP=1
KINGDOMS_HTTP_TOKEN=$TOKEN
KINGDOMS_HTTP_BIND=127.0.0.1:8742
KINGDOMS_HTTP_WEBHOOK=https://war.example.com/kingdoms/events
KINGDOMS_HTTP_WEBHOOK_SECRET=$TOKEN      # optional; defaults to the bearer token
```

**Tunnel.** `cloudflared` ingress to the bind address only; do not expose 8742 on a public NIC.

```yaml
# ~/.cloudflared/config.yml
tunnel: <tunnel-id>
credentials-file: /home/pumpkin/.cloudflared/<tunnel-id>.json
ingress:
  - hostname: kingdoms.example.com
    service: http://127.0.0.1:8742
  - service: http_status:404
```

Sails then calls `https://kingdoms.example.com` with `Authorization: Bearer $TOKEN`.

**Webhook signing.** Every POST to `KINGDOMS_HTTP_WEBHOOK` has header `X-Kingdoms-Signature: sha256=<hex>` over the raw body bytes (HMAC-SHA256, secret = `KINGDOMS_HTTP_WEBHOOK_SECRET` or the bearer token). Verify the bytes you received; do not re-encode the JSON. Reject 401/413/429 the same way the server does (token, body > 16 KiB, > 20 req/s).

```python
import hmac, hashlib
def check(secret: str, body: bytes, header: str) -> bool:
    expect = "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expect, header.strip().lower())
```

`kingdoms-mock-director` is the in-repo client (`GET /war`, `POST /raids`, webhook listener).

**Director loop.** Poll `/war` (clock stays on), pick a quiet owned town, `POST /raids`, then wait for `town_captured` / `raid_ended` on the webhook before the next front:

```bash
BASE=https://kingdoms.example.com
AUTH="Authorization: Bearer $TOKEN"
# 1. front-line map
curl -sS -H "$AUTH" "$BASE/war"
# 2. open a front (pauses the in-game clock for 30 s)
curl -sS -H "$AUTH" -H 'content-type: application/json' \
  -d '{"town":"20261025:0:0","size":40,"side":"pillager"}' "$BASE/raids"
# 3. optional: name a marshal, shout, stop
curl -sS -H "$AUTH" -H 'content-type: application/json' \
  -d '{"kingdom":"alder","name":"Sir Aldric","title":"marshal","line":"Hold the gate."}' \
  "$BASE/heroes"
curl -sS -H "$AUTH" -X POST "$BASE/raids/1/stop"
```

On Sails, run that in a 2–5 s loop: `GET /war` → if `fronts` is under the cap and a town has `raid_active: false`, POST a raid; on webhook `town_captured` or `gate_breached`, shout via `POST /heroes/<kingdom>/shout`. Full JSON shapes are in the contract below.

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
curl -sS -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"kingdom":"alder","name":"Sir Aldric","title":"marshal","line":"Hold the gate."}' \
  http://127.0.0.1:8742/heroes
curl -sS -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"text":"Alder to the wall."}' http://127.0.0.1:8742/heroes/alder/shout
```

Sails can drive that contract with `kingdoms-mock-director` (GET `/war`, choose a town, POST `/raids`, optional webhook listener that checks `X-Kingdoms-Signature`):

```bash
cargo run -p pumpkin-kingdoms --bin kingdoms-mock-director -- \
  --base http://127.0.0.1:8742 --token "$TOKEN" --town 0,0 --listen 127.0.0.1:8743
```

Observational GETs (`/war`, `/lines`, `/heroes`) authenticate but **do not** pause the war clock. Everything else that succeeds with a bearer token does.

### HTTP and webhook contract (Sails war-map)

Auth: `Authorization: Bearer <KINGDOMS_HTTP_TOKEN>`. 401 if missing/wrong. 413 if body > 16 KiB. 429 if more than 20 requests/second.

Clock: a successful auth on a non-observational route pauses the war clock for 30 s (`clock.overridden` on `GET /war`). Observational: `GET /war`, `GET|POST /lines`, `GET|POST /heroes`.

#### `GET /war`

Front-line snapshot. Does not pause the clock.

```json
{
  "tick": 4800,
  "scores": { "alder": 2, "raven": 1, "pillager": 3 },
  "clock": { "enabled": true, "period": 2400, "overridden": false, "last_tick": 2400 },
  "next_raid_id": 8,
  "fronts": 3,
  "heroes": [{ "kingdom": "alder", "name": "Sir Aldric", "title": "marshal", "line": "Hold the gate." }],
  "towns": [{
    "id": "20261025:0:0",
    "kingdom": "alder",
    "owner": "alder",
    "alarm": true,
    "raid_active": true,
    "gate_breached": false,
    "keep": { "x": 1268, "z": 665 },
    "gate": { "x": 1504, "z": 597 },
    "occupy_hold": 40,
    "occupy_need": 3,
    "defenders": 20,
    "raiders": 40,
    "raid_side": "pillager",
    "repair_until": 0,
    "live": true,
    "flag": { "x": 1268, "z": 665 },
    "flag_status": "capturing",
    "flag_paused": false,
    "flag_attackers": 4,
    "flag_defenders": 0
  }]
}
```

#### `GET /state`

Pauses the clock. Towns use the battle schema (`id`, `kingdom`, `owner`, `alarm`, `raid_active`, `wave`, `waves`, `defenders_alive`, `raiders_alive`, `defenders_killed`, `raiders_killed`, `threatened_gate`, `gate_breached`, `occupy_hold`, `occupy_need`, `ram_hp`, `flag`, `flag_status`, `flag_paused`, `flag_attackers`, `flag_defenders`) plus logged-in players and troop totals.

```json
{
  "tick": 4800,
  "towns": [{ "id": "20261025:0:0", "kingdom": "alder", "owner": "alder", "alarm": true, "raid_active": true, "wave": 0, "waves": 1, "defenders_alive": 20, "raiders_alive": 40, "defenders_killed": 0, "raiders_killed": 0, "threatened_gate": [1504, 597], "gate_breached": false, "occupy_hold": 0, "occupy_need": 3, "ram_hp": 80.0, "flag": { "x": 1268, "z": 665 }, "flag_status": "idle", "flag_paused": false, "flag_attackers": 0, "flag_defenders": 0 }],
  "players": [{ "name": "alder1", "uuid": "937abb5d-9fc8-61f3-9ebc-41f786b9d29c", "kingdom": "alder", "x": 1504.5, "y": 80.0, "z": 597.5 }],
  "troops": { "defenders": 20, "raiders": 40 },
  "war_clock": true
}
```

#### `GET /towns/{id}`

Pauses the clock. One battle object as in `/state`. 404 if unknown.

#### `POST /raids`

Pauses the clock. Body: `{ "town": "0,0"|"here"|"20261025:0:0", "size": 40, "defenders": 20, "side": "pillager"|"kingdom"|"alder"|"raven", "camp": null }`. `camp` is reserved. 200 `{ "id": 1, "town": "20261025:0:0", "size": 40, "side": "pillager" }`. 409 if a raid is already running.

#### `POST /raids/{id}/stop`

Pauses the clock. `{ "stopped": 1, "id": 1 }`.

#### `GET|POST /lines`

Does not pause the clock. GET `{ "lines": { "gate_breached": "...", "rally": "...", "retreat": "...", "victory": "...", "charge": "...", "hold": "...", "fallen": "..." } }`. POST `{ "event": "rally", "text": "Alder to the gate." }` → `{ "ok": true, "event": "rally", "text": "Alder to the gate." }`.

#### `GET|POST /heroes`

Does not pause the clock. GET `{ "heroes": [{ "kingdom": "alder", "name": "Sir Aldric", "title": "marshal", "line": "Hold the gate." }] }`. POST `{ "kingdom": "alder", "name": "Sir Aldric", "title": "marshal", "line": "Hold the gate." }` (alias `text` for `line`).

#### `POST /heroes/{kingdom}/shout`

Pauses the clock. `{ "text": "Alder to the wall." }` (alias `message`). 200 `{ "ok": true, "hero": { ... } }`. 404 if that kingdom has no hero.

#### `POST /chat`

Pauses the clock. `{ "name": "Herald", "message": "The gate is down" }` → `{ "ok": true }`. Broadcasts `[Herald] The gate is down` to every player.

#### Webhooks

`POST` JSON to `KINGDOMS_HTTP_WEBHOOK`. Header `X-Kingdoms-Signature: sha256=<hmac-sha256 hex of the raw body>` using `KINGDOMS_HTTP_WEBHOOK_SECRET` (defaults to the bearer token). Shared envelope unless noted:

```json
{ "event": "raid_started", "tick": 1600, "actor": 1, "other": 40, "x": 1551.0, "z": 485.0, "note": "20261025:0:0 raid 1 size 40" }
```

| `event` | when | notes |
| --- | --- | --- |
| `raid_started` | a raid is armed | `actor` is the raid id, `other` is size |
| `town_alarm` | a town first rings | |
| `gate_breached` | ram clears the portcullis | `other` is bars cleared |
| `portcullis_dropped` | gates close on alarm | `other` is gate count |
| `trebuchet_fired` | director or player looses a shot | |
| `ladders_raised` | assault ladders placed on the wall | `actor` is blocks placed |
| `ram_destroyed` | defenders wreck the ram | |
| `rout` | a side breaks | |
| `capture_started` | attackers on the plate begin the hold | `actor` is on-plate count |
| `capture_progress` | hold advances (every 40 ticks) | `actor` is hold ticks, `other` is need |
| `capture_paused` | a defender is on or near the plate | |
| `flag_changed` | the pole banner swaps on flip | |
| `town_captured` | occupy or retake hold completes | capture and retake share this name; `note` says who |
| `war_clock` | the clock starts a front | |
| `player_joined_a_kingdom` | `/kingdom join` | `{ "event", "player", "kingdom" }` only |

Examples (same envelope, HMAC over the raw bytes):

```json
{ "event": "raid_started", "tick": 1600, "actor": 1, "other": 40, "x": 1551.0, "z": 485.0, "note": "20261025:0:0 raid 1 size 40" }
{ "event": "town_alarm", "tick": 1601, "actor": 0, "other": null, "x": 1504.0, "z": 597.0, "note": "20261025:0:0 alarm" }
{ "event": "portcullis_dropped", "tick": 1601, "actor": 0, "other": 2, "x": 1504.0, "z": 597.0, "note": "20261025:0:0 portcullis dropped" }
{ "event": "trebuchet_fired", "tick": 1680, "actor": 0, "other": null, "x": 1551.0, "z": 485.0, "note": "20261025:0:0 trebuchet loosed" }
{ "event": "ladders_raised", "tick": 1720, "actor": 6, "other": null, "x": 1498.0, "z": 590.0, "note": "20261025:0:0 assault ladders raised" }
{ "event": "gate_breached", "tick": 1900, "actor": 12, "other": 8, "x": 1504.0, "z": 597.0, "note": "20261025:0:0 ram breached 8 bars" }
{ "event": "ram_destroyed", "tick": 1880, "actor": 12, "other": null, "x": 1490.0, "z": 590.0, "note": "20261025:0:0 ram wrecked" }
{ "event": "rout", "tick": 2000, "actor": 40, "other": 8, "x": 1504.0, "z": 597.0, "note": "20261025:0:0 pillagers rout" }
{ "event": "town_captured", "tick": 2400, "actor": 12, "other": 8, "x": 1268.0, "z": 665.0, "note": "20261025:0:0 captured by pillager" }
{ "event": "war_clock", "tick": 2400, "actor": 2, "other": 24, "x": 1551.0, "z": 485.0, "note": "war clock raid 2 on 20261025:1:0" }
{ "event": "player_joined_a_kingdom", "player": "alder1", "kingdom": "alder" }
```

Kills, waves, and victory shouts stay in-game; they are not webhooked. Verify HMAC on the raw bytes, not a re-encoded object.

`kingdoms-ownership.json` now also stores kingdom scores, `clock_tick`, `next_raid_id`, `last_clock_tick`, and per-player stats (kills, captures, defenses, banner) so a restart keeps the war. Town ownership and player membership were already there.

Civilians work at a workstation by day (ticks 2000–11000) and go home to a bed at night. The town bell still sends them to the keep when a raid starts. Guards patrol the wall between raids. Empty bed slots refill at most two citizens per 200 ticks, and never during a fight.

### Population

`ChunkPopulate` (PR #1) spawns a stable garrison: roughly 4–16 guards, 4–12 archers, 8–24 civilians, 4 builders, 2 golems, homes a function of the plan. Citizens are `PersistenceRequired` and tagged (`kingdoms:citizen`, `kingdoms:role:…`). Empty slots respawn if the home chunk is loaded. `/raid start 200` is 200 **raiders** against that garrison, not 200 extra villagers. Use the siege bench for N vs N.

### Scale / LOD

On each director tick: spatial buckets (16 blocks) for target acquisition. Idle garrisons still `set_no_ai` stagger; a live raid does not, because skipping AI also skipped walking. Path-search cap 48/tick applies only to **squad leaders** refreshing live `navigator.set_progress`. Everyone else steers on a shared flow field (one BFS per gate/keep, 4-block cells, O(1) lookup). Followers keep cohesion with their leader; within ~16 blocks and LOS they walk straight at the target.

Fronts with no player within 128 blocks of keep, gate, or camp stay **abstract**: virtual raider/defender counts, a 400-tick march, then Lanchester combat on a 20-tick timer. Entities and force-load tickets appear only when a player approaches (and the live body budget has room). `GET /war` reports `live` so the map can tell a materialised fight from a statistical one.

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

Round 6 (seed `20261025`, release, LTO off, two `kingdoms-raid-bot`s at gate/keep). Alarm uses the walled town (keep/gates/wall + 24), not the hinterland camp, so leftover palisade pillagers do not keep the bell ringing. Routed stragglers outside that rect despawn; tagged raiders despawn when no raid is running. The war clock waits `hold_lockout + repair_window` (800 ticks) and can open alder/raven fronts on roads. Captains shout to nearby players (rate-limited); `POST /lines` supplies the text. `/war` prints scores and captures.

| Fight | peak bodies | fight MSPT | director_ms | `Can't keep up` | notes |
| --- | ---: | ---: | ---: | --- | --- |
| r6 120 vs 40 flip | **170** ticking (40/130 at first contact) | **12–16** (spawn **29.21**, then 12–16) | **3.5–5.3** | join only **8.99 s / 179 ticks**. No raid-spawn hitch ≥ 50 ms after warmup | ram breached **21**; `keep_r=8 hold=200` **captured** tick 3167; leftover despawned 82; `repair_until=3951` |
| r6 clock retake 80 | **177** (98/79) | **13–20** (peak **20.39** at spawn) | **3.6–4.6** | none during retake | clock Kingdom raid tick **5000** (after repair window); `defenders_in_keep=8` **retaken** tick 7723 owner=alder; scores alder=1 pillager=1 |
| r6 30 min soak (3 fronts, 100+100 each) | **747** director bodies / **1331** world entities | **p50 60.6 / p95 101 / max 290** (join 558) | 20–38 steady | 3-bot join **43.6 s / 871 ticks**, then catch-up 80 `Can't keep up` while MSPT stayed ~60 | alder captured `20261025:0:1`; raven marched on `0:0`; 3 concurrent fronts; tickets 158→481 (max 514, no leak); RSS 68→810 MiB (chunk cache, not a ticket leak); bodies 747→169 |

Capture: `kingdoms town captured … score=pillager` then webhook `town_captured` / `retaken by alder`. Clock did **not** fire on the flip tick (`repair_until=3951`, clock at 5000). `POST /lines` set rally to `Alder to the gate. Hold fast.`; captains shouted rally / gate_breached / victory to the gate bot. `/war` and `GET /war` showed owner, scores, raid_side, repair_until.

Player-join hitch (investigate only, no pumpkin-core change): `chunker::update_position` on spawn diffs an empty watched section against the full view-distance cylinder (`Cylindrical::changed_chunks`) and, on that same tick, (1) adds view + simulation tickets for every chunk, (2) `enqueue_chunk`s the whole set onto `ChunkSender`, (3) `mark_chunks_as_newly_watched` + `spawn_world_entity_chunks`. `ticker.rs` then logs `Can't keep up` (~8.5–9.0 s / ~175–179 ticks at view distance 16).

Plots: `/opt/cursor/artifacts/live-r6.log`, `live-r6-strip.png` (3990×1024), `live-r6-clock-strip.png`, `soak-r6.csv`, `kingdoms-ownership-r6.json`, `webhook-r6.log`.

Round 7 is a performance pass. Remote fronts (no player within 128 blocks of keep/gate/camp) no longer spawn soldiers or force-load towns: they keep virtual counts and resolve Lanchester combat on a 20-tick timer after a 400-tick march. `KINGDOMS_BODY_BUDGET` (default 240) caps live bodies; the war clock still opens up to `KINGDOMS_WAR_CLOCK_FRONTS` raids, but extras stay abstract. Live tickets cover the battle line only (1-chunk ring, 8-step corridor). Camp pillagers inside `plan.bounds` no longer hold the alarm. Join hitch: r7 added `set_join_view_grow(1)` in pumpkin-core; a warm-world join was still 8.9 s (town-plan ensure), so r8 reverts both pumpkin-core hooks.

| Fight | peak bodies | fight MSPT | director_ms | `Can't keep up` | notes |
| --- | ---: | ---: | ---: | --- | --- |
| r6 30 min soak (before) | **747** / 1331 entities | **p50 60.6 / p95 101 / max 290** (join 558) | 20–38 | 3-bot join 43.6 s / 871 ticks | 3 live 100 vs 100 fronts |
| r7 30 min soak (3 fronts + 1 bot) | **230** live / 556 entities | **p50 34.2 / p95 47.4 / max 65** after join (join sample 522, 40 s first-plan hitch) | p50 **3.5** / p95 92 (PNG dumps whenever `KINGDOMS_PLOT_DIR` is set) | 8 (join 41 s / 819 ticks; later plot dumps 2 s) | 1 live + 3 abstract; tickets 0→87 (no leak); RSS 66→618 MiB; **9 captures, 7 kingdom** (scores alder 4 / raven 3 / pillager 2) |

Target was p50 ≤ 25 / p95 ≤ 45. Director time is 3.5 ms; the leftover ~30 ms is the live player's chunk/entity tick, not 3×100 vs 100. First join still hitches on the first town plan (~40 s director), not the view-distance fill. Three bots were still connected 30 s into that hitch. A later join on the same world took 2.6 s. Kingdom occupy of a pillager town was stuck at `occupy_hold >> 200` until `tick_hold` started treating keep-holds as a capture for any owner.

Plots: `/opt/cursor/artifacts/live-r7.log`, `soak-r7.csv`, `soak-r7-summary.json`, `kingdoms-ownership-r7.json`, `webhook-r7.log`.

Round 8: town plans precompute on a background thread at world attach (spawn cell ±1 and owned fronts) and persist `TownInfo` JSON under `kingdoms-plans/v2/` (`PLAN_CACHE_VERSION`; bump it when plan layout changes). The tick never calls `plan_for`; a miss queues work and defers that town's AI. Idle civilians/guards farther than 64 blocks from every player sleep (`NoAI`); kingdoms-tagged mobs never run vanilla villager brains; workstation/home/wall Y uses planner height instead of per-entity `sample_surface_height`. Join-view-grow pumpkin-core hooks are reverted (r7 warm-world join was still 8.9 s of town-plan ensure).

| Fight | peak bodies | fight MSPT | director_ms | `Can't keep up` | notes |
| --- | ---: | ---: | ---: | --- | --- |
| r7 30 min soak (before) | **230** live / 556 entities | **p50 34.2 / p95 47.4 / max 65** after join (join sample 522, 40 s first-plan hitch) | p50 **3.5** / p95 92 | 8 (join 41 s / 819 ticks) | 1 live + 3 abstract; 9 captures, 7 kingdom |
| r8 30 min soak (after warmup) | **256** live / 836 entities | **p50 34.9 / p95 49.4 / max 71** after join | p50 **0.26** / p95 **1.48** (max 902 on a dual-capture tick) | 14 (join catch-up 2.2 s / 43 ticks; later 2 s bursts) | warmup 9 cells in **78 s**; 9 JSON plans on disk; **36 captures, 25 kingdom / 11 pillager**; tickets 0→85 (no leak); RSS 88→733 MiB |

First join after warmup no longer runs `plan_for` on the tick (r7 director **40 s**). Worst director on that join was **154 ms** (ensure + first civilian scan). Target “no tick over 100 ms” still misses that 154 ms join director and raid-start bursts (554 ms spawning the first live wave). Steady p50 **34.9** vs target **25**: director is 0.26 ms; the leftover ~34 ms is vanilla chunk/entity tick of the live player in the PR #17 capital (entity loop `loop_us≈300`, ~60/165 villagers asleep). Kingdom occupy of pillager towns resolves (`tick_hold` + `occupy`).

Plots: `/opt/cursor/artifacts/soak-r8.log`, `soak-r8.csv`, `soak-r8-summary.json`, `kingdoms-ownership-r8.json`, `webhook-r8.log`.

Round 9 spreads leftover director spikes and puts players in the war. Soldiers spawn 3 per town (4 across the world); ram, extras, and wave take turns; rout/rally tags cap at 8; leftover despawns and ticket drops drain 12/tick; ownership JSON writes off-thread; the war clock starts one front per tick; repair scans one column per tick and skips the post-flip lockout. Players earn persisted renown (kills + 5×captures + 3×defenses) with kit tiers (leather → chain → iron) and a kingdom banner at banneret. `/kingdom fronts` lists battles; `/kingdom deploy` teleports to a friendly rally and materialises an abstract front on the existing warmup. Soldiers shout rate-limited contextual lines; `POST /heroes` lets Sails name a commander per kingdom.

It did not hit “no director tick ≥ 50 ms”. Leftover spikes are `mark_chunks_as_newly_watched` while applying live raid tickets (4/tick still 170–530 ms), a 1163 ms live-front tick during first contact, and dual abstract captures (760 ms when two towns flipped on the same tick, down from 902). War-clock raid 1 was 2 ms. Spawn batches are 3. Captains/soldiers shouted 197 times. Player stats persist but the standing bots scored 0 kills. `/kingdom deploy` was not in-game tested.

Plots: `/opt/cursor/artifacts/soak-r9.log`, `soak-r9.csv`, `soak-r9-summary.json`, `kingdoms-ownership-r9.json`, `webhook-r9.log`.

| Fight | peak bodies | fight MSPT | director_ms | `Can't keep up` | notes |
| --- | ---: | ---: | ---: | --- | --- |
| r8 30 min soak (before) | **256** live / 836 entities | **p50 34.9 / p95 49.4 / max 71** after join | p50 **0.26** / p95 **1.48** (max 902 dual-capture) | 14 | 36 captures; raid-start 554 / rout 708 / dual-capture 902 |
| r9 30 min soak | **266** live / 942 entities | **p50 39.7 / p95 53.1 / max 73** after join | p50 **0.64** / p95 **1.66** (max **1163** on a live-ticket tick) | 12 | 1 live + 3 abstract; **38 captures** (alder 11 / raven 18 / pillager 9); tickets 0→86; RSS 88→848 MiB; **16 director ticks ≥ 50 ms** (log); soldier shouts 197; pumpkin-core untouched |

Round 9b (same branch, r8 config: 1 bot, 3 fronts, 30 min) re-soaked this branch against master `6166fc32` in parallel. Capture bookkeeping is one flip per tick; raid tickets are 1/tick with async chunk fetch (no `mark_chunks_as_newly_watched` on the director tick); approach/deploy pre-tickets a 256-block ring. Both soaks ran on the same machine with a live-test server, so MSPT is higher than the single-server r8 34.9 and is only comparable to each other.

| | branch (this PR) | master `6166fc32` |
| --- | ---: | ---: |
| MSPT p50 / p95 / max | 44.04 / 61.49 / 79.04 | 44.51 / 64.24 / 88.23 |
| director_ms p50 / p95 / max | **0.39 / 0.62 / 2.07** | 0.46 / 0.73 / 4.07 |
| director ticks ≥ 50 ms | **0** | **0** |
| captures | 30 | 29 |
| `Can't keep up` | 37 | 43 |
| tickets max | 85 | 86 |
| live bodies max | 247 | 258 |
| soldier shouts | 187 | 89 |

This branch is not slower than master on the same config. The r8→r9 p50 34.9→39.7 was the 3-bot r9 soak, not rank/chat/spreading. Dual-capture 760 ms and first-contact 1163 ms did not recur (`director_max` 2.07).

`kingdoms-raid-bot --fight` joined alder, listed `/kingdom fronts`, deployed to the threatened gate (`TELEPORT 1504.5, 80, 597.5`, not the keep roof), melee-hit pillagers (entity 104) and witches, and ranked up: `You are now a freeman (renown 10).` then `/war stats` `rank=freeman renown=17 kills=17`. Freeman kit is leather plus a stone sword (`kit_player` on that event). Soldier chat on the same run: `The gate is down. Through the breach!` / `To the walls. Hold the line.` Two abstract towns flipped in the same second with `director_ms=0.71` (capture queue). Gaps: levy still has no weapon until freeman; keep respawn sometimes dumps the bot at world spawn `(10.5, 75)` after `inWall`; inventory packets were not dumped so the stone sword is inferred from the rank-up kit path.

Plots: `/opt/cursor/artifacts/soak-r9b-branch.csv`, `soak-r8-master.csv`, `raid-bot-fight.log`, `raid-bot-fight-live.txt`.

Round 10 (this PR) gives levy a wooden sword and alder/raven a shield on join and respawn, picks a standable keep courtyard (then the nearest friendly town) instead of a wall or world spawn, and dumps kit packets from the raid bot. Camps grow a player-usable trebuchet (director cadence or `/kingdom fire` / anvil) whose cobble arc punches wall holes repair can later fill; a walking siege tower and camp-side ladders let raiders scale the curtain. Archers stand on the wall walk and towers, a third of the guards sally at the ram, and the alarm drops the portcullis (iron bars, webhooked). `KINGDOMS.md` now has the full HTTP and webhook contract for a Sails war-map.

Live `kingdoms-raid-bot --prove` after merging master `c0b8f38a` (seed `20261025`, alder, capital `0,0`):

- Kit: `KIT-PROVE slot slot=36 wooden_sword` and slot 45 `shield` (plus leather). Server `kingdoms levy kit … weapon=wooden_sword shield=shield`.
- `/kill` respawn: `kingdoms keep spawn player=alder1 x=1256 y=165 z=691` then `TELEPORT 1256.5, 165, 691.5`. Keep mass is `1255..1294 × 653..684` with roof ~173; this is south of the keep, 8 below the roof, not world spawn `(10.5, 75)`.
- Trebuchet: `loosed` / `impact` / `trebuchet hole x=1504 y=65 z=535` (town-gate column, not the keep). Repair: `repair-by-diff refilled trebuchet hole placed=30 town=20261025:0:0` (then 38, 45, …). No `skipped keep` lines.
- Tower + ladders: `siege tower spawned`, `assault ladders raised`, webhook `ladders_raised`. Curtain: `raider crossed curtain id=91 x=1275.3 y=161.0 z=682.8`.
- Archers: `archer on wall walk id=193 x=1561.5 y=79.0 z=495.5`. Ram sally: `ram sally id=60 x=1327.5 z=669.5`. Portcullis: `gate closed x=1504 z=597` and `x=1327 z=669`; webhook `portcullis_dropped` on the short prove runs.

r8 config side-by-side (1 bot, 3 fronts, 30 min, seed `20261025`) vs master `c0b8f38a` (PR #22). Sequential, same machine, not overlapped.

| | branch (this PR) | master `c0b8f38a` |
| --- | ---: | ---: |
| MSPT p50 / p95 / max | 34.09 / **42.27** / 343.79 | 33.86 / 44.64 / 331.71 |
| director_ms p50 / p95 / max | 0.24 / **0.36** / **1.17** | 0.21 / 0.53 / 5.76 |
| director ticks ≥ 50 ms (CSV) | **0** | **0** |
| hitch log (director_ms) | 1 (`tick=23119` **629.10**, mspt 35.12) | 0 |
| captures | 30 | 30 |
| `Can't keep up` | **2** | 7 |
| tickets max | 85 | 85 |
| live bodies max | 262 | 252 |
| soldier shouts | 193 | 183 |

Steady MSPT is within noise of master (p50 +0.23, p95 better). Director p95/max and `Can't keep up` are better. The one 629 ms director hitch did not move that tick's MSPT (35.12). Tickets did not leak (85 end = max).

Plots: `/opt/cursor/artifacts/soak-r23-branch.csv`, `soak-r23-master.csv`, `raid-bot-prove-live-final.txt`, `soak-r23-branch.log`.

Round 11 (this PR) splits the tick-23119 629 ms punch-repair dump: trebuchet holes and plan refill both apply **2 blocks/tick**, per-block hole logs are gone, and any director phase over 20 ms logs `kingdoms phase tick=… phase=… ms=…`. Targeting reuses one `SpatialBuckets::nearest_hostile` scan; followers `slide_direct` instead of refreshing a path. Militia (even entity ids) take a wooden sword from the armoury and line the inner gate; others carry buckets to the well; named `captain` / `blacksmith` / `priest` shout on the bell. `/kingdom rally [follow|hold|charge]` and a goat horn (Freeman+) order nearby bot soldiers. `kingdoms-mock-director` plus `sails_http_contract_war_raid_and_hmac` drive GET `/war`, POST `/raids`, and HMAC `X-Kingdoms-Signature` for Sails.

3-front raid-bot soak (seed `20261025`, bots at `(0,0)` / `(1,0)` / `(0,1)` gates, `/raid start here 80 50`, `KINGDOMS_BODY_BUDGET=400`, debug, sequential vs master `14ed819d`):

| | branch (8 min) | branch (3 min, 2-block punch) | master `14ed819d` (3 min) |
| --- | ---: | ---: | ---: |
| MSPT p50 / p95 / max | 53.67 / 67.29 / 112.03 | **54.48 / 68.28 / 102.73** | 67.38 / 77.37 / 111.12 |
| director_ms p50 / p95 / max (CSV) | **0.79 / 2.25 / 8.05** | 2.21 / 3.10 / 8.10 | 1.26 / 4.08 / 12.20 |
| director ticks ≥ 50 ms (CSV) | **0** | **0** | **0** |
| hitch log max (director_ms) | 471.59 (ticket apply, town `1:0`) | 290.54 (first-contact tickets) | 177.71 (first-contact tickets) |
| phase > 20 ms (engines / towns max) | 118 / 471 | 110 / 290 | n/a (no phase log) |
| live fronts max | 2 | 2 | **3** |
| live bodies max | 220 | 224 | 270 |
| `Can't keep up` | 24 | **9** | 10 |
| soldier shouts | 379 | 93 | 26 |

The 629 ms repair dump is gone from the CSV (director max 8 ms). Remaining hitches are chunk-ticket apply on first contact (phase `town:…`) and `set_block_state(NOTIFY)` for two punch/ladder blocks (~50–110 ms engines, down from 45 writes). Bottleneck toward 300 live bodies is entity/chunk sim (MSPT ~55 with ~330 entities) plus ticket apply, not targeting or follower pathing (director p50 0.8–2.2 ms with 220 live). Three raids started; two fronts stayed materialized on the branch soaks (third had tickets but 0 ticking bodies). Master held three live fronts and peaked at 270 bodies. Named NPC chat on the bell: `speaker="captain"|"blacksmith"|"priest"`. Squad orders need a Freeman in-game (levy bots cannot `/kingdom rally`).

Plots: `/opt/cursor/artifacts/soak-r11-branch-summary.json`, `soak-r11-branch2-summary.json`, `soak-r11-master-summary.json`.

Round 12 captured at the keep flag plinth, not by clearing the whole inner bailey. Attackers stand on the plate; defenders within 4 blocks pause; hiders in walls do not. After the ram breaches or eight raiders storm the portcullis, the column paths through the gatehouse to the plate. Distant combatants skip targeting outside 48 blocks of a player; materialized raids snapshot by entity id; item/XP orbs despawn 8/tick; engines run on one town per tick. The first 40 live ticks of a town skip engines and repair scan. `/kingdom rank set` is permission level 2+. `PLAN_CACHE_VERSION` is 13. The plinth sits on the paved keep-hall court from PR #32. `KINGDOMS.md` has a Sails quickstart (token, cloudflared, HMAC, director loop).

10-min identical-load soak (seed `20261025`, 3 fronts, `/raid start` 120 40, `KINGDOMS_BODY_BUDGET=600`, debug, plan cache v10) vs master `7bab6500` (v9). The branch has since been rebased onto `28f9bee7f` (PRs #31–#33) with `PLAN_CACHE_VERSION` 13; the soak numbers below are still that v10-vs-v9 run. The plinth now sits on PR #32's paved keep-hall court. War clock off so the three 120+40 gate raids stay the load. After the ram / storm breach, this branch paths the column through the gatehouse onto the plate.

| | branch (this PR) | master `7bab6500` |
| --- | ---: | ---: |
| MSPT p50 / p95 / max | **81.69** / 103.72 / 114.11 | 102.66 / 146.55 / 190.43 |
| director_ms p50 / p95 / max | **2.74** / 10.27 / 155.62 | 8.30 / 16.27 / 32.07 |
| first-town-tick hitch (director_ms) | **214.5** (`towns` 214.35) | **625.18** (`towns` 624.84) |
| live fronts / bodies max | 3 / **480** | 3 / 456 |
| captures / plate_r max | **2** / **109** | 0 / 0 (keep_r max 94; soak still skips hold) |
| `Can't keep up` | 28 | 45 |

p50 is still above the 50 ms budget; leftover cost is vanilla entity/chunk sim of ~480 NoAI soldiers. Two of the three 120-raider sieges walked the gatehouse, stacked on the plate (`plate_r=109` and `60`, need=6), and flipped. Capital `0:0` stayed at the town gate (`keep_r=0`) for that run.

A shorter war-clock pass (size 24, three fronts) also reached the plate: `plate_r=4`, three flips, hitch 0.

Plots: `/opt/cursor/artifacts/soak-r12-branch480-summary.json`, `soak-r12-master480-summary.json`, `soak-r12-branch-clock24-summary.json`. Flag crops: `flag-plinth-se.png`, `flag-plinth-se-flipped.png`, `flag-plinth-court.png`.

Round 13 (this PR): capital sieges walk outer town → civic → castle → gatehouse → plate (flow-field rings, no phantom join wall). Defenders within 48 blocks of the plate rush it on `capture_started`; the captain still shouts "To the flag!". The mast is eight fences with a real banner, a 3×2 wool panel, and lit braziers at the pedestal corners. Distant soldiers skip targeting outside 64 blocks of a player; alarmed far combatants still slide on the Alert lod; the first 40 live ticks AI-loop at most 64 bodies and defer portcullis writes. Tower and ram displays spawn two parts per tick, trebuchet/ladder/punch/ram-bar writes stay at two per tick, and the first live ticks prewarm engine columns so the opening siege-engine dump cannot hitch 200 ms. Kingdoms raid bodies set `director_driven` so pumpkin-core skips living movement only for those mobs; a plain `/summon` NoAI still falls and collides. Idle towns snapshot one X-strip per tick; ticket warmup prewarms a flow field instead of walking the town AABB. `PLAN_CACHE_VERSION` stays 15 (stamp-only flag, rebased onto PR #36).

Release identical-load soak, three runs each side (seed `20261025`, 3 fronts, `/raid start` 120 40, `KINGDOMS_BODY_BUDGET=600`, `CARGO_PROFILE_RELEASE_LTO=off`, plan cache v15) vs master `f22e6487`. Values are mean (min–max):

| | branch (this PR) | master `f22e6487` |
| --- | ---: | ---: |
| MSPT p50 | **65.44 (61.39–71.36)** | 67.69 (61.55–73.08) |
| MSPT p95 | **78.54 (76.07–82.99)** | 85.17 (77.66–91.47) |
| first-town hitch (director_ms) | **159 (152–168)** | 214 (162–288) |
| captures | 1.33 (0–3) | **1.67 (1–2)** |
| plate_r max | 39 (0–60) | **43 (42–45)** |

p50 is still above the 50 ms budget; leftover cost is vanilla entity/chunk sim of ~480 packed soldiers. Branch mean p50 and hitch are under master (hitch range 152–168 vs master's 162–288). Capture count overlaps: two branch runs flipped 1 and 3 towns with plate_r 57 and 60, one run never reached the plate (`plate_r=0`). Master never had a zero-capture run. Charge shouts (`kind=charge`) fired on the capturing towns.

Plots: `/opt/cursor/artifacts/soak-r15-variance.json`, `soak-r13-branch{1,2,3}-summary.json`, `soak-r13-master{1,2,3}-summary.json`. Flag crops: `flag-plinth-se.png`, `flag-plinth-se-flipped.png`, `flag-plinth-court.png`.

### Commands

| Command | |
| --- | --- |
| `/locatetown` | nearest planned town |
| `/kingdom info` | town, kingdom, owner, battle counts, gate breach |
| `/kingdom join <kingdom>` | `alder` or `raven`. Rank kit, name tag, friendly fire off, respawn at the keep |
| `/kingdom fronts` | active battles (side, owner, live, counts) |
| `/kingdom deploy [town]` | teleport to the threatened gate (live surface Y, not the keep roof); abstract fronts materialise on a staggered warmup |
| `/kingdom stats` | your rank, renown, kills, captures, defenses |
| `/kingdom fire` | loose a camp trebuchet at the threatened wall (stand at the anvil) |
| `/kingdom rally [follow\|hold\|charge]` | Freeman+ orders nearby bot soldiers (default follow). A goat horn or `/kingdom horn` cycles the same orders. |
| `/kingdom rank set <rank>` | op: grant levy/freeman/sergeant/knight/banneret without farming kills |
| `/raid start <town> [size] [defenders]` | start a pillager raid. Optional `defenders` is how many kingdom combatants to field (extras spawn at the threatened gate). |
| `/raid retake <town> [size]` | kingdom counter-raid to retake a pillager-owned town |
| `/raid stop` | stop all kingdoms raids |
| `/war` | scores, clock, owners, active fronts and raid sides |
| `/war stats` | same as `/kingdom stats` |

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
