//! Headless N-vs-N siege on a planned town.
//!
//! This is not a full Pumpkin world: no block collision, no vanilla navigator.
//! Movement uses the same shared flow field the live director caches (one BFS
//! per gate/keep). `PathBudget` is only spent on squad-leader searches; everyone
//! else steers along the field for free. Numbers are honest about that.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use pumpkin_data::entity::EntityType;

use crate::faction::Faction;
use crate::flow::{DIRECT_RANGE, FlowField, SQUAD_SIZE, is_squad_leader};
use crate::lod::{ACQUIRE_RANGE, DEFAULT_PATH_CAP, LodTier, PathBudget, SpatialBuckets};
use crate::raid::compose_wave;
use crate::roles::VillagerRole;
use crate::scene::{ScenePost, TownScene};

const MELEE_RANGE: f64 = 2.5;
const ARROW_RANGE: f64 = 16.0;
const GUARD_HP: f32 = 20.0;
const ARCHER_HP: f32 = 16.0;
const CIVILIAN_HP: f32 = 20.0;
const PILLAGER_HP: f32 = 24.0;
const VINDICATOR_HP: f32 = 24.0;
const MELEE_DAMAGE: f32 = 4.0;
const ARROW_DAMAGE: f32 = 3.0;
const MARCH_STEP: f64 = 0.34;
const COMBAT_STEP: f64 = 0.28;
const FOLLOW_RANGE: f64 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimKind {
    Guard,
    Archer,
    Civilian,
    Builder,
    Pillager,
    Vindicator,
}

impl SimKind {
    #[must_use]
    pub const fn role(self) -> Option<VillagerRole> {
        match self {
            Self::Guard => Some(VillagerRole::Guard),
            Self::Archer => Some(VillagerRole::Archer),
            Self::Civilian => Some(VillagerRole::Civilian),
            Self::Builder => Some(VillagerRole::Builder),
            Self::Pillager | Self::Vindicator => None,
        }
    }

    #[must_use]
    pub const fn hp(self) -> f32 {
        match self {
            Self::Guard => GUARD_HP,
            Self::Archer => ARCHER_HP,
            Self::Civilian | Self::Builder => CIVILIAN_HP,
            Self::Pillager => PILLAGER_HP,
            Self::Vindicator => VINDICATOR_HP,
        }
    }

    #[must_use]
    pub const fn ranged(self) -> bool {
        matches!(self, Self::Archer | Self::Pillager)
    }
}

#[derive(Clone, Debug)]
pub struct SimEntity {
    pub id: i32,
    pub kind: SimKind,
    pub faction: Faction,
    pub x: f64,
    pub z: f64,
    pub hp: f32,
    pub cooldown: i32,
    pub home: ScenePost,
    pub alive: bool,
}

#[derive(Clone, Debug)]
pub struct SimReport {
    pub ticks: u32,
    pub defenders: usize,
    pub raiders: usize,
    pub defenders_left: usize,
    pub raiders_left: usize,
    pub defender_kills: u32,
    pub raider_kills: u32,
    pub ms_per_tick: f64,
    pub worst_ms: f64,
    pub total_ms: f64,
    pub path_attempts: u64,
    pub path_capped: u64,
    pub melee_engaged: u32,
    pub peak_melee: u32,
    pub snapshots: Vec<SimSnapshot>,
}

#[derive(Clone, Debug)]
pub struct SimSnapshot {
    pub tick: u32,
    pub positions: Vec<(i32, f64, f64, bool)>,
}

pub struct SiegeSim {
    pub entities: Vec<SimEntity>,
    pub alarm: bool,
    pub threatened: ScenePost,
    pub keep: ScenePost,
    pub tick: u32,
    pub path_attempts: u64,
    pub path_capped: u64,
    pub defender_kills: u32,
    pub raider_kills: u32,
    pub melee_engaged: HashSet<i32>,
    pub peak_melee: u32,
    gate_field: FlowField,
    keep_field: FlowField,
    next_id: i32,
}

impl SiegeSim {
    #[must_use]
    pub fn from_scene(scene: &dyn TownScene, defenders: usize, raiders: usize) -> Self {
        let kingdom = Faction::Kingdom(scene.kingdom());
        let keep = scene.keep();
        let threatened = scene
            .gates()
            .first()
            .map_or(keep, |g| ScenePost::new(g.x, g.z));
        let camp = scene
            .raid_camps()
            .first()
            .copied()
            .unwrap_or_else(|| ScenePost::new(keep.x, keep.z + 80));
        let mut sim = Self {
            entities: Vec::with_capacity(defenders + raiders),
            alarm: raiders > 0,
            threatened,
            keep,
            tick: 0,
            path_attempts: 0,
            path_capped: 0,
            defender_kills: 0,
            raider_kills: 0,
            melee_engaged: HashSet::new(),
            peak_melee: 0,
            gate_field: FlowField::toward(scene, threatened, &[camp]),
            keep_field: FlowField::toward(scene, keep, &[camp]),
            next_id: 1,
        };
        let homes = scene.civilian_homes();
        let combat = (defenders * 2 / 3).max(1);
        let n_archers = combat / 3;
        let n_guards = combat.saturating_sub(n_archers);
        // Alarm is already up: garrison holds the threatened gate while
        // raiders march in from the hinterland camp. Spreading 200 guards
        // around a 400-block wall never meets in one fight.
        let guard_spread = ((n_guards.max(1) as f64).sqrt().ceil() as i32).max(6);
        let archer_spread = ((n_archers.max(1) as f64).sqrt().ceil() as i32).max(4);
        for i in 0..n_guards {
            sim.push(
                SimKind::Guard,
                kingdom.clone(),
                ring_around(threatened, i, guard_spread),
            );
        }
        for i in 0..n_archers {
            sim.push(
                SimKind::Archer,
                kingdom.clone(),
                ring_around(threatened, i + n_guards, archer_spread),
            );
        }
        for i in n_guards + n_archers..defenders {
            let home = *homes.get(i % homes.len().max(1)).unwrap_or(&keep);
            sim.push(SimKind::Civilian, kingdom.clone(), home);
        }
        let members = compose_wave(raiders as u16, 0);
        for (i, member) in members.iter().enumerate() {
            let kind = if member.entity_type == &EntityType::VINDICATOR {
                SimKind::Vindicator
            } else {
                SimKind::Pillager
            };
            let ring = i as i32;
            let home = ScenePost::new(camp.x + (ring % 12) - 6, camp.z + (ring / 12) - 6);
            sim.push(kind, Faction::Pillager, home);
        }
        sim
    }

    fn push(&mut self, kind: SimKind, faction: Faction, home: ScenePost) {
        let id = self.next_id;
        self.next_id += 1;
        self.entities.push(SimEntity {
            id,
            kind,
            faction,
            x: f64::from(home.x) + 0.5,
            z: f64::from(home.z) + 0.5,
            hp: kind.hp(),
            cooldown: 0,
            home,
            alive: true,
        });
    }

    pub fn tick(&mut self, budget: &PathBudget) {
        self.tick += 1;
        let mut buckets = SpatialBuckets::new();
        for e in &self.entities {
            if !e.alive {
                continue;
            }
            buckets.insert(crate::lod::BucketEntry {
                entity_id: e.id,
                x: e.x,
                z: e.z,
                faction: e.faction.clone(),
            });
        }

        let mut leaders: HashMap<u32, (f64, f64, i32)> = HashMap::new();
        for e in &self.entities {
            if !e.alive {
                continue;
            }
            let squad = squad_key(e);
            let entry = leaders.entry(squad).or_insert((e.x, e.z, e.id));
            if e.id < entry.2 {
                *entry = (e.x, e.z, e.id);
            }
        }

        let mut kills: Vec<(i32, bool)> = Vec::new();
        let mut melee_this_tick = 0u32;
        let count = self.entities.len();
        for i in 0..count {
            if !self.entities[i].alive {
                continue;
            }
            let alarm = self.alarm;
            let threatened = self.threatened;
            let keep = self.keep;
            let e = &self.entities[i];
            let hostile = buckets
                .nearest_hostile(e.x, e.z, &e.faction, ACQUIRE_RANGE)
                .map(|h| h.entity_id);
            let hostile_pos = hostile.and_then(|hid| {
                self.entities
                    .iter()
                    .find(|o| o.id == hid && o.alive)
                    .map(|o| (o.x, o.z))
            });
            let in_melee =
                hostile_pos.is_some_and(|(tx, tz)| dist(e.x, e.z, tx, tz) <= MELEE_RANGE);
            if in_melee {
                melee_this_tick += 1;
                self.melee_engaged.insert(e.id);
            }

            let dest = if let Some((tx, tz)) = hostile_pos {
                ScenePost::new(tx.floor() as i32, tz.floor() as i32)
            } else if matches!(
                e.kind,
                SimKind::Pillager | SimKind::Vindicator | SimKind::Guard
            ) && alarm
            {
                threatened
            } else if e.kind == SimKind::Civilian && alarm {
                keep
            } else {
                e.home
            };

            let dest_x = f64::from(dest.x) + 0.5;
            let dest_z = f64::from(dest.z) + 0.5;
            let close = dist(e.x, e.z, dest_x, dest_z) <= DIRECT_RANGE;

            let (wx, wz) = if let Some((tx, tz)) = hostile_pos {
                // Already in a fight: charge the target, do not wait on the leader.
                (tx, tz)
            } else if close {
                (dest_x, dest_z)
            } else {
                let (wx, wz) = self.waypoint_for(dest, e.x, e.z);
                // A blocked-cell field can send units the long way around the
                // wall. This sim has no collision, so never follow a waypoint
                // that is farther from the objective than a straight line.
                let (wx, wz) =
                    if dist(wx, wz, dest_x, dest_z) + 1.0 < dist(e.x, e.z, dest_x, dest_z) {
                        (wx, wz)
                    } else {
                        (dest_x, dest_z)
                    };
                if is_squad_leader(e.id) {
                    (wx, wz)
                } else if let Some(&(lx, lz, lid)) = leaders.get(&squad_key(e))
                    && lid != e.id
                    && dist(e.x, e.z, lx, lz) > FOLLOW_RANGE
                    && dist(lx, lz, dest_x, dest_z) + 4.0 < dist(e.x, e.z, dest_x, dest_z)
                {
                    (lx, lz)
                } else {
                    let slot = (e.id.unsigned_abs() % SQUAD_SIZE) as f64;
                    (wx + (slot % 3.0 - 1.0) * 1.2, wz + (slot / 3.0 - 1.0) * 1.2)
                }
            };

            // Flow-field / local steering is free. Only a far squad leader
            // consumes the live-navigator analog budget.
            let leader_search = is_squad_leader(e.id) && !close;
            if leader_search {
                self.path_attempts += 1;
                if !budget.try_acquire() {
                    self.path_capped += 1;
                }
            }
            let step = if in_melee || hostile.is_some() {
                COMBAT_STEP
            } else {
                MARCH_STEP
            };
            let (x, z) = steer_by(e.x, e.z, wx, wz, step);
            self.entities[i].x = x;
            self.entities[i].z = z;
            if let Some((tx, tz)) = hostile_pos
                && dist(x, z, tx, tz) <= MELEE_RANGE
            {
                if !in_melee {
                    melee_this_tick += 1;
                }
                self.melee_engaged.insert(self.entities[i].id);
            }

            let tier = if hostile.is_some() {
                LodTier::Combat
            } else if alarm {
                LodTier::Alert
            } else {
                LodTier::Idle
            };
            if !tier.should_tick(self.entities[i].id, self.tick) {
                continue;
            }

            if self.entities[i].cooldown > 0 {
                self.entities[i].cooldown -= 1;
                continue;
            }
            let Some(hid) = hostile else {
                continue;
            };
            let Some(ti) = self.entities.iter().position(|o| o.id == hid && o.alive) else {
                continue;
            };
            let d = dist(
                self.entities[i].x,
                self.entities[i].z,
                self.entities[ti].x,
                self.entities[ti].z,
            );
            let ranged = self.entities[i].kind.ranged();
            let reach = if ranged { ARROW_RANGE } else { MELEE_RANGE };
            if d > reach {
                continue;
            }
            if !ranged {
                self.melee_engaged.insert(self.entities[i].id);
                self.melee_engaged.insert(self.entities[ti].id);
            }
            let dmg = if ranged { ARROW_DAMAGE } else { MELEE_DAMAGE };
            self.entities[ti].hp -= dmg;
            self.entities[i].cooldown = if ranged { 20 } else { 12 };
            if self.entities[ti].hp <= 0.0 {
                self.entities[ti].alive = false;
                let victim_is_raider = matches!(
                    self.entities[ti].kind,
                    SimKind::Pillager | SimKind::Vindicator
                );
                kills.push((self.entities[ti].id, victim_is_raider));
            }
        }
        for (_, raider) in kills {
            if raider {
                self.raider_kills += 1;
            } else {
                self.defender_kills += 1;
            }
        }
        if melee_this_tick > self.peak_melee {
            self.peak_melee = melee_this_tick;
        }
        self.alarm = self
            .entities
            .iter()
            .any(|e| e.alive && matches!(e.kind, SimKind::Pillager | SimKind::Vindicator));
    }

    fn waypoint_for(&self, dest: ScenePost, x: f64, z: f64) -> (f64, f64) {
        if dest == self.keep {
            self.keep_field.waypoint(x, z)
        } else if dest == self.threatened {
            self.gate_field.waypoint(x, z)
        } else {
            (f64::from(dest.x) + 0.5, f64::from(dest.z) + 0.5)
        }
    }

    pub fn run(&mut self, ticks: u32, snapshot_every: u32) -> SimReport {
        let defenders = self
            .entities
            .iter()
            .filter(|e| e.faction.is_kingdom())
            .count();
        let raiders = self
            .entities
            .iter()
            .filter(|e| matches!(e.faction, Faction::Pillager))
            .count();
        let budget = PathBudget::new(DEFAULT_PATH_CAP);
        let mut snapshots = vec![self.snapshot()];
        let mut worst = 0.0f64;
        let start = Instant::now();
        for _ in 0..ticks {
            budget.reset();
            let t0 = Instant::now();
            self.tick(&budget);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            if ms > worst {
                worst = ms;
            }
            if snapshot_every > 0 && self.tick.is_multiple_of(snapshot_every) {
                snapshots.push(self.snapshot());
            }
        }
        if snapshots.last().is_none_or(|s| s.tick != self.tick) {
            snapshots.push(self.snapshot());
        }
        let total_ms = start.elapsed().as_secs_f64() * 1000.0;
        SimReport {
            ticks,
            defenders,
            raiders,
            defenders_left: self
                .entities
                .iter()
                .filter(|e| e.alive && e.faction.is_kingdom())
                .count(),
            raiders_left: self
                .entities
                .iter()
                .filter(|e| e.alive && matches!(e.faction, Faction::Pillager))
                .count(),
            defender_kills: self.defender_kills,
            raider_kills: self.raider_kills,
            ms_per_tick: total_ms / f64::from(ticks.max(1)),
            worst_ms: worst,
            total_ms,
            path_attempts: self.path_attempts,
            path_capped: self.path_capped,
            melee_engaged: self.melee_engaged.len() as u32,
            peak_melee: self.peak_melee,
            snapshots,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> SimSnapshot {
        SimSnapshot {
            tick: self.tick,
            positions: self
                .entities
                .iter()
                .filter(|e| e.alive)
                .map(|e| {
                    (
                        e.id,
                        e.x,
                        e.z,
                        matches!(e.kind, SimKind::Pillager | SimKind::Vindicator),
                    )
                })
                .collect(),
        }
    }
}

fn dist(ax: f64, az: f64, bx: f64, bz: f64) -> f64 {
    let dx = ax - bx;
    let dz = az - bz;
    dx.hypot(dz)
}

fn steer_by(x: f64, z: f64, tx: f64, tz: f64, step: f64) -> (f64, f64) {
    let dx = tx - x;
    let dz = tz - z;
    let len = dx.hypot(dz);
    if len < step {
        return (tx, tz);
    }
    (x + dx / len * step, z + dz / len * step)
}

fn ring_around(center: ScenePost, i: usize, width: i32) -> ScenePost {
    let i = i as i32;
    let w = width.max(1);
    ScenePost::new(center.x + i.rem_euclid(w) - w / 2, center.z + i / w - w / 2)
}

fn squad_key(e: &SimEntity) -> u32 {
    let side = if matches!(e.kind, SimKind::Pillager | SimKind::Vindicator) {
        1_000_000
    } else {
        0
    };
    side + e.id.unsigned_abs().saturating_sub(1) / SQUAD_SIZE
}

const PLOT_MIN: u32 = 1024;
const LEGEND_W: u32 = 220;
const GROUND: image::Rgb<u8> = image::Rgb([34, 40, 32]);
const WALL: image::Rgb<u8> = image::Rgb([118, 118, 112]);
const KEEP: image::Rgb<u8> = image::Rgb([92, 64, 36]);
const GATE: image::Rgb<u8> = image::Rgb([210, 186, 64]);
const RAIDER: image::Rgb<u8> = image::Rgb([196, 48, 42]);
const DEFENDER: image::Rgb<u8> = image::Rgb([64, 148, 220]);
const PANEL: image::Rgb<u8> = image::Rgb([24, 26, 22]);
const INK: image::Rgb<u8> = image::Rgb([230, 230, 222]);

/// Draw a top-down PNG of the last snapshot (at least 1024 px, with legend).
pub fn write_plot(
    scene: &dyn TownScene,
    report: &SimReport,
    path: &std::path::Path,
) -> std::io::Result<()> {
    let snap = report.snapshots.last();
    render_frame(scene, snap, report, "END", path)
}

/// Start / mid / end frames plus a horizontal strip. Returns the paths written.
pub fn write_plot_snapshots(
    scene: &dyn TownScene,
    report: &SimReport,
    dir: &std::path::Path,
    n: usize,
) -> std::io::Result<Vec<std::path::PathBuf>> {
    write_named_snapshots(scene, &report.snapshots, dir, &format!("siege-{n}"))
}

/// Same frames as [`write_plot_snapshots`], with a caller-chosen file stem.
pub fn write_named_snapshots(
    scene: &dyn TownScene,
    snapshots: &[SimSnapshot],
    dir: &std::path::Path,
    stem: &str,
) -> std::io::Result<Vec<std::path::PathBuf>> {
    write_named_snapshots_with_kills(scene, snapshots, dir, stem, 0, 0)
}

/// Live plots: start/mid/end from captured entity positions.
pub fn write_named_snapshots_with_kills(
    scene: &dyn TownScene,
    snapshots: &[SimSnapshot],
    dir: &std::path::Path,
    stem: &str,
    defender_kills: u32,
    raider_kills: u32,
) -> std::io::Result<Vec<std::path::PathBuf>> {
    let report = SimReport {
        ticks: snapshots.last().map_or(0, |s| s.tick),
        defenders: 0,
        raiders: 0,
        defenders_left: 0,
        raiders_left: 0,
        defender_kills,
        raider_kills,
        ms_per_tick: 0.0,
        worst_ms: 0.0,
        total_ms: 0.0,
        path_attempts: 0,
        path_capped: 0,
        melee_engaged: 0,
        peak_melee: 0,
        snapshots: snapshots.to_vec(),
    };
    let snaps = pick_keyframes(&report);
    let labels = ["START", "MID", "END"];
    let mut paths = Vec::new();
    let mut frames = Vec::new();
    for (label, snap) in labels.iter().zip(snaps.iter()) {
        let path = dir.join(format!("{stem}-{}.png", label.to_ascii_lowercase()));
        let img = render_image(scene, snap.as_ref(), &report, label);
        img.save(&path).map_err(std::io::Error::other)?;
        frames.push(img);
        paths.push(path);
    }
    if let Some(strip) = stitch_h(&frames) {
        let path = dir.join(format!("{stem}-strip.png"));
        strip.save(&path).map_err(std::io::Error::other)?;
        paths.push(path);
    }
    Ok(paths)
}

fn pick_keyframes(report: &SimReport) -> [Option<SimSnapshot>; 3] {
    let snaps = &report.snapshots;
    if snaps.is_empty() {
        return [None, None, None];
    }
    let last = snaps.len() - 1;
    let mid = last / 2;
    [
        Some(snaps[0].clone()),
        Some(snaps[mid].clone()),
        Some(snaps[last].clone()),
    ]
}

fn render_frame(
    scene: &dyn TownScene,
    snap: Option<&SimSnapshot>,
    report: &SimReport,
    label: &str,
    path: &std::path::Path,
) -> std::io::Result<()> {
    render_image(scene, snap, report, label)
        .save(path)
        .map_err(std::io::Error::other)
}

fn render_image(
    scene: &dyn TownScene,
    snap: Option<&SimSnapshot>,
    report: &SimReport,
    label: &str,
) -> image::RgbImage {
    let bounds = scene.bounds();
    let mut min_x = bounds.min_x;
    let mut min_z = bounds.min_z;
    let mut max_x = bounds.max_x;
    let mut max_z = bounds.max_z;
    for p in scene.raid_camps() {
        min_x = min_x.min(p.x);
        min_z = min_z.min(p.z);
        max_x = max_x.max(p.x);
        max_z = max_z.max(p.z);
    }
    if let Some(snap) = snap {
        for (_, x, z, _) in &snap.positions {
            min_x = min_x.min(x.floor() as i32);
            min_z = min_z.min(z.floor() as i32);
            max_x = max_x.max(x.floor() as i32);
            max_z = max_z.max(z.floor() as i32);
        }
    }
    let pad = 24;
    min_x -= pad;
    min_z -= pad;
    max_x += pad;
    max_z += pad;
    let map_w = (max_x - min_x).max(1) as u32;
    let map_h = (max_z - min_z).max(1) as u32;
    let scale = ((PLOT_MIN as f64 / f64::from(map_w.max(map_h))).ceil() as u32).max(1);
    let img_w = map_w * scale + LEGEND_W;
    let img_h = (map_h * scale).max(PLOT_MIN);
    let mut img = image::RgbImage::from_pixel(img_w, img_h, GROUND);

    let to_px =
        |x: i32, z: i32| -> (i32, i32) { ((x - min_x) * scale as i32, (z - min_z) * scale as i32) };
    let thick = (scale as i32).max(2);

    // Town wall under the dots.
    let wall = scene.wall();
    for pair in wall.windows(2) {
        let (x0, z0) = to_px(pair[0][0], pair[0][1]);
        let (x1, z1) = to_px(pair[1][0], pair[1][1]);
        draw_line(&mut img, x0, z0, x1, z1, WALL, thick);
    }
    if wall.len() >= 2 {
        let a = wall[0];
        let b = wall[wall.len() - 1];
        let (x0, z0) = to_px(a[0], a[1]);
        let (x1, z1) = to_px(b[0], b[1]);
        draw_line(&mut img, x0, z0, x1, z1, WALL, thick);
    }

    let keep = scene.keep();
    let (kx, kz) = to_px(keep.x, keep.z);
    let keep_r = (8 * scale as i32).max(10);
    fill_rect(
        &mut img,
        kx - keep_r,
        kz - keep_r,
        kx + keep_r,
        kz + keep_r,
        KEEP,
    );

    for g in scene.gates() {
        let (gx, gz) = to_px(g.x, g.z);
        let gr = (3 * scale as i32).max(5);
        fill_rect(&mut img, gx - gr, gz - gr, gx + gr, gz + gr, GATE);
    }

    if let Some(snap) = snap {
        let dot = (scale as i32).max(2);
        for (_, x, z, raider) in &snap.positions {
            let (px, pz) = to_px(x.floor() as i32, z.floor() as i32);
            let color = if *raider { RAIDER } else { DEFENDER };
            fill_rect(&mut img, px - dot, pz - dot, px + dot, pz + dot, color);
        }
    }

    let lx = (map_w * scale) as i32;
    fill_rect(&mut img, lx, 0, img_w as i32, img_h as i32, PANEL);
    let mut y = 24;
    blit_text(&mut img, lx + 16, y, label, INK, 2);
    y += 36;
    let tick = snap.map_or(report.ticks, |s| s.tick);
    blit_text(&mut img, lx + 16, y, &format!("TICK {tick}"), INK, 2);
    y += 28;
    blit_swatch(&mut img, lx + 16, y, RAIDER);
    blit_text(&mut img, lx + 40, y, "RAIDERS", INK, 2);
    y += 22;
    blit_swatch(&mut img, lx + 16, y, DEFENDER);
    blit_text(&mut img, lx + 40, y, "DEFENDERS", INK, 2);
    y += 22;
    blit_swatch(&mut img, lx + 16, y, WALL);
    blit_text(&mut img, lx + 40, y, "WALL", INK, 2);
    y += 22;
    blit_swatch(&mut img, lx + 16, y, KEEP);
    blit_text(&mut img, lx + 40, y, "KEEP", INK, 2);
    y += 22;
    blit_swatch(&mut img, lx + 16, y, GATE);
    blit_text(&mut img, lx + 40, y, "GATE", INK, 2);
    y += 36;
    blit_text(
        &mut img,
        lx + 16,
        y,
        &format!("MELEE {}", report.melee_engaged),
        INK,
        2,
    );
    y += 22;
    blit_text(
        &mut img,
        lx + 16,
        y,
        &format!("PEAK {}", report.peak_melee),
        INK,
        2,
    );
    y += 22;
    blit_text(
        &mut img,
        lx + 16,
        y,
        &format!("DKILL {}", report.defender_kills),
        INK,
        2,
    );
    y += 22;
    blit_text(
        &mut img,
        lx + 16,
        y,
        &format!("RKILL {}", report.raider_kills),
        INK,
        2,
    );

    img
}

fn stitch_h(frames: &[image::RgbImage]) -> Option<image::RgbImage> {
    use image::RgbImage;
    frames.first()?;
    let w: u32 = frames.iter().map(RgbImage::width).sum();
    let h = frames.iter().map(RgbImage::height).max()?;
    let mut out = RgbImage::from_pixel(w, h, GROUND);
    let mut x = 0u32;
    for frame in frames {
        for (px, py, p) in frame.enumerate_pixels() {
            if py < h {
                out.put_pixel(x + px, py, *p);
            }
        }
        x += frame.width();
    }
    Some(out)
}

fn fill_rect(img: &mut image::RgbImage, x0: i32, y0: i32, x1: i32, y1: i32, color: image::Rgb<u8>) {
    let min_x = x0.min(x1).max(0);
    let min_y = y0.min(y1).max(0);
    let max_x = x0.max(x1).min(img.width() as i32 - 1);
    let max_y = y0.max(y1).min(img.height() as i32 - 1);
    if max_x < min_x || max_y < min_y {
        return;
    }
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            img.put_pixel(x as u32, y as u32, color);
        }
    }
}

fn draw_line(
    img: &mut image::RgbImage,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: image::Rgb<u8>,
    thick: i32,
) {
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        fill_rect(
            img,
            x - thick / 2,
            y - thick / 2,
            x + thick / 2,
            y + thick / 2,
            color,
        );
        if x == x1 && y == y1 {
            break;
        }
        let e2 = err * 2;
        if e2 > -dy {
            err -= dy;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            y += sy;
        }
    }
}

fn blit_swatch(img: &mut image::RgbImage, x: i32, y: i32, color: image::Rgb<u8>) {
    fill_rect(img, x, y, x + 14, y + 14, color);
}

fn blit_text(img: &mut image::RgbImage, x: i32, y: i32, text: &str, color: image::Rgb<u8>, s: i32) {
    let mut cx = x;
    for ch in text.chars() {
        if let Some(glyph) = glyph(ch) {
            for row in 0i32..7 {
                for col in 0i32..5 {
                    if glyph[row as usize] & (1 << (4 - col)) != 0 {
                        fill_rect(
                            img,
                            cx + col * s,
                            y + row * s,
                            cx + col * s + s - 1,
                            y + row * s + s - 1,
                            color,
                        );
                    }
                }
            }
        }
        cx += 6 * s;
    }
}

const fn glyph(ch: char) -> Option<[u8; 7]> {
    Some(match ch {
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1F],
        '3' => [0x0E, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0E, 0x11, 0x10, 0x0E, 0x01, 0x11, 0x0E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faction::KingdomId;
    use crate::height::HeightGrid;
    use crate::info::TownInfo;
    use crate::palette::PaletteKind;
    use crate::plan::build_plan;
    use crate::scene::TownInfoScene;
    use crate::town::town_in_cell;

    fn scene() -> TownInfoScene {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            70,
            8,
            4,
        );
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        TownInfoScene::from_info(TownInfo::from_plan(&plan))
    }

    #[test]
    fn fifty_vs_fifty_runs_and_scores_kills() {
        let scene = scene();
        let mut sim = SiegeSim::from_scene(&scene, 50, 50);
        assert_eq!(sim.entities.len(), 100);
        let report = sim.run(400, 50);
        assert_eq!(report.defenders, 50);
        assert_eq!(report.raiders, 50);
        assert!(report.ms_per_tick < 50.0, "ms/tick {}", report.ms_per_tick);
        assert!(
            report.melee_engaged > 10,
            "expected a real melee: engaged={} kills={}/{}",
            report.melee_engaged,
            report.defender_kills,
            report.raider_kills
        );
    }

    #[test]
    fn eighty_vs_eighty_closes_to_melee() {
        let scene = scene();
        let mut sim = SiegeSim::from_scene(&scene, 80, 80);
        let report = sim.run(600, 50);
        let gate = sim.threatened;
        let mut raider_d = Vec::new();
        let mut guard_d = Vec::new();
        for e in &sim.entities {
            if !e.alive {
                continue;
            }
            let d = (e.x - f64::from(gate.x)).hypot(e.z - f64::from(gate.z));
            if matches!(e.kind, SimKind::Pillager | SimKind::Vindicator) {
                raider_d.push(d);
            } else if e.kind == SimKind::Guard {
                guard_d.push(d);
            }
        }
        raider_d.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        guard_d.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let rmin = raider_d.first().copied().unwrap_or(0.0);
        let gmin = guard_d.first().copied().unwrap_or(0.0);
        let gmed = guard_d.get(guard_d.len() / 2).copied().unwrap_or(0.0);
        assert!(
            report.melee_engaged >= 40 && report.peak_melee >= 16,
            "melee engaged={} peak={} capped={}/{} raider_min={rmin:.1} guard={gmin:.1}/{gmed:.1}",
            report.melee_engaged,
            report.peak_melee,
            report.path_capped,
            report.path_attempts
        );
        assert!(
            report.path_capped * 4 < report.path_attempts.max(1) || report.path_attempts < 8_000,
            "leader path cap should not reject the fight: capped={}/{}",
            report.path_capped,
            report.path_attempts
        );
    }

    #[test]
    fn targeting_picks_enemies_not_allies() {
        let alder = Faction::Kingdom(KingdomId::new("alder"));
        let mut buckets = SpatialBuckets::new();
        buckets.insert(crate::lod::BucketEntry {
            entity_id: 1,
            x: 0.0,
            z: 0.0,
            faction: alder.clone(),
        });
        buckets.insert(crate::lod::BucketEntry {
            entity_id: 2,
            x: 3.0,
            z: 0.0,
            faction: alder.clone(),
        });
        buckets.insert(crate::lod::BucketEntry {
            entity_id: 3,
            x: 5.0,
            z: 0.0,
            faction: Faction::Pillager,
        });
        let hit = buckets.nearest_hostile(0.0, 0.0, &alder, 32.0).unwrap();
        assert_eq!(hit.entity_id, 3);
    }
}
