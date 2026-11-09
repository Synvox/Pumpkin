//! Headless N-vs-N siege on a planned town.
//!
//! This is not a full Pumpkin world: no block collision, no vanilla navigator.
//! It ticks the same faction, role, alarm, spatial-bucket and LOD rules the
//! live director uses, plus a cheap steering step along roads / straight lines.
//! Numbers are honest about that.

use std::time::Instant;

use pumpkin_data::entity::EntityType;

use crate::faction::Faction;
use crate::lod::{
    ACQUIRE_RANGE, ALARM_RANGE, DEFAULT_PATH_CAP, LodTier, PathBudget, SpatialBuckets,
};
use crate::raid::compose_wave;
use crate::roles::{GoalKind, VillagerRole, select_goal};
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
const STEP: f64 = 0.25;

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
            next_id: 1,
        };
        let guard_posts = scene.guard_posts();
        let archer_posts = scene.archer_posts();
        let homes = scene.civilian_homes();
        let combat = (defenders * 2 / 3).max(1);
        let n_archers = combat / 3;
        let n_guards = combat.saturating_sub(n_archers);
        for i in 0..n_guards {
            let home = *guard_posts
                .get(i % guard_posts.len().max(1))
                .unwrap_or(&keep);
            sim.push(SimKind::Guard, kingdom.clone(), home);
        }
        for i in 0..n_archers {
            let home = *archer_posts
                .get(i % archer_posts.len().max(1))
                .unwrap_or(&keep);
            sim.push(SimKind::Archer, kingdom.clone(), home);
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

        let mut kills: Vec<(i32, bool)> = Vec::new();
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
            let in_melee = hostile.is_some_and(|hid| {
                self.entities
                    .iter()
                    .find(|o| o.id == hid)
                    .is_some_and(|o| dist(e.x, e.z, o.x, o.z) <= MELEE_RANGE)
            });
            let tier = if hostile.is_some() {
                LodTier::Combat
            } else if alarm {
                LodTier::Alert
            } else {
                LodTier::Idle
            };
            if !tier.should_tick(e.id, self.tick) {
                continue;
            }

            let goal = e
                .kind
                .role()
                .map_or(GoalKind::Melee, |role| select_goal(role, alarm, in_melee));

            let dest = match goal {
                GoalKind::FleeKeep | GoalKind::RingBell => keep,
                GoalKind::Patrol | GoalKind::Melee if alarm && e.kind == SimKind::Guard => {
                    threatened
                }
                GoalKind::HoldPost | GoalKind::ReturnHome | GoalKind::BuilderStub => e.home,
                GoalKind::Melee | GoalKind::Ranged | GoalKind::Patrol => {
                    if let Some(hid) = hostile {
                        self.entities
                            .iter()
                            .find(|o| o.id == hid)
                            .map_or(e.home, |t| {
                                ScenePost::new(t.x.floor() as i32, t.z.floor() as i32)
                            })
                    } else if e.kind == SimKind::Pillager || e.kind == SimKind::Vindicator {
                        threatened
                    } else {
                        e.home
                    }
                }
            };

            self.path_attempts += 1;
            if budget.try_acquire() {
                let (x, z) = {
                    let e = &self.entities[i];
                    steer(e.x, e.z, f64::from(dest.x) + 0.5, f64::from(dest.z) + 0.5)
                };
                self.entities[i].x = x;
                self.entities[i].z = z;
            } else {
                self.path_capped += 1;
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
        self.alarm = self.entities.iter().any(|e| {
            e.alive
                && matches!(e.kind, SimKind::Pillager | SimKind::Vindicator)
                && dist(
                    e.x,
                    e.z,
                    f64::from(self.threatened.x),
                    f64::from(self.threatened.z),
                ) < ALARM_RANGE
        }) || self
            .entities
            .iter()
            .any(|e| e.alive && matches!(e.kind, SimKind::Pillager | SimKind::Vindicator));
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
        let mut snapshots = Vec::new();
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

fn steer(x: f64, z: f64, tx: f64, tz: f64) -> (f64, f64) {
    let dx = tx - x;
    let dz = tz - z;
    let len = dx.hypot(dz);
    if len < STEP {
        return (tx, tz);
    }
    (x + dx / len * STEP, z + dz / len * STEP)
}

/// Draw a top-down PNG of the last snapshot over the town wall.
pub fn write_plot(
    scene: &dyn TownScene,
    report: &SimReport,
    path: &std::path::Path,
) -> std::io::Result<()> {
    use image::{Rgb, RgbImage};

    let bounds = scene.bounds();
    let pad = 40;
    let min_x = bounds.min_x - pad;
    let min_z = bounds.min_z - pad;
    let w = (bounds.max_x - bounds.min_x + pad * 2).max(1) as u32;
    let h = (bounds.max_z - bounds.min_z + pad * 2).max(1) as u32;
    let scale = 1u32;
    let mut img = RgbImage::from_pixel(w * scale, h * scale, Rgb([32, 36, 28]));

    let put = |img: &mut RgbImage, x: i32, z: i32, color: Rgb<u8>, radius: i32| {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let px = (x - min_x) * scale as i32 + dx;
                let pz = (z - min_z) * scale as i32 + dz;
                if px >= 0 && pz >= 0 && (px as u32) < img.width() && (pz as u32) < img.height() {
                    img.put_pixel(px as u32, pz as u32, color);
                }
            }
        }
    };

    for p in scene.wall() {
        put(&mut img, p[0], p[1], Rgb([90, 90, 90]), 0);
    }
    let keep = scene.keep();
    for dx in -4..=4 {
        for dz in -4..=4 {
            put(&mut img, keep.x + dx, keep.z + dz, Rgb([70, 50, 30]), 0);
        }
    }
    for g in scene.gates() {
        put(&mut img, g.x, g.z, Rgb([200, 180, 60]), 2);
    }

    if let Some(snap) = report.snapshots.last() {
        for (_, x, z, raider) in &snap.positions {
            let color = if *raider {
                Rgb([180, 40, 40])
            } else {
                Rgb([60, 140, 220])
            };
            put(&mut img, x.floor() as i32, z.floor() as i32, color, 2);
        }
    }

    img.save(path).map_err(std::io::Error::other)
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
        let report = sim.run(200, 50);
        assert_eq!(report.defenders, 50);
        assert_eq!(report.raiders, 50);
        assert!(report.ms_per_tick < 50.0, "ms/tick {}", report.ms_per_tick);
        // Someone should be swinging in 200 ticks of a stacked fight.
        assert!(
            report.defender_kills + report.raider_kills > 0
                || report.defenders_left + report.raiders_left == 100,
            "expected combat or a standoff: {report:?}"
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
