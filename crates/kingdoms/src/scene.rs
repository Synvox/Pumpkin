//! Town gameplay points from [`TownInfo::spawns`].
//!
//! Hinterland planning writes villager beds, workstations, wall/tower guard
//! posts, market/keep rallies, gates, and pillager camp spawn/rally columns.
//! [`TownInfoScene`] is a thin adapter so the director and siege bench keep a
//! stable `TownScene` trait.

use crate::faction::KingdomId;
use crate::geom::{Rect, dist2};
use crate::info::{GateInfo, SpawnPoint, SpawnRole, TownInfo};

/// A column a defender or civilian should care about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScenePost {
    pub x: i32,
    /// Planner standing height (`surface + 1`). `0` means “sample live”.
    pub y: i32,
    pub z: i32,
}

impl ScenePost {
    #[must_use]
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, y: 0, z }
    }

    #[must_use]
    pub const fn with_y(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub const fn from_tuple(p: (i32, i32)) -> Self {
        Self {
            x: p.0,
            y: 0,
            z: p.1,
        }
    }

    #[must_use]
    pub const fn from_xz(p: [i32; 2]) -> Self {
        Self {
            x: p[0],
            y: 0,
            z: p[1],
        }
    }

    #[must_use]
    pub const fn from_spawn(p: SpawnPoint) -> Self {
        Self {
            x: p.x,
            y: p.y,
            z: p.z,
        }
    }

    #[must_use]
    pub const fn as_tuple(self) -> (i32, i32) {
        (self.x, self.z)
    }

    /// Planner Y, or `fallback` when this post was built without a spawn.
    #[must_use]
    pub const fn height_or(self, fallback: i32) -> i32 {
        if self.y == 0 { fallback } else { self.y }
    }
}

/// Geometry a combat director needs. Implement this for richer scene dumps later.
pub trait TownScene {
    fn town_id(&self) -> &str;
    fn kingdom(&self) -> KingdomId;
    fn bounds(&self) -> Rect;
    fn keep(&self) -> ScenePost;
    fn capture_point(&self) -> ScenePost;
    fn gates(&self) -> &[GateInfo];
    fn guard_posts(&self) -> &[ScenePost];
    fn archer_posts(&self) -> &[ScenePost];
    fn civilian_homes(&self) -> &[ScenePost];
    fn workstations(&self) -> &[ScenePost];
    fn rally_points(&self) -> &[ScenePost];
    fn raid_camps(&self) -> &[ScenePost];
    fn ram_posts(&self) -> &[ScenePost];
    fn ladder_posts(&self) -> &[ScenePost];
    fn trebuchet_posts(&self) -> &[ScenePost];
    fn road_approach(&self) -> Option<ScenePost>;
    fn wall(&self) -> &[[i32; 2]];
    fn keep_rect(&self) -> Rect;
    fn gatehouse_rect(&self) -> Rect;

    /// Gate nearest to a world column. Used when pillagers approach.
    fn nearest_gate(&self, x: i32, z: i32) -> Option<GateInfo> {
        self.gates()
            .iter()
            .copied()
            .min_by_key(|g| dist2(g.x, g.z, x, z))
    }

    fn threatened_rally(&self, x: i32, z: i32) -> ScenePost {
        if let Some(gate) = self.nearest_gate(x, z) {
            return ScenePost::new(gate.x, gate.z);
        }
        self.rally_points()
            .iter()
            .copied()
            .min_by_key(|p| dist2(p.x, p.z, x, z))
            .unwrap_or_else(|| self.keep())
    }
}

/// Adapter over [`TownInfo::spawns`].
#[derive(Clone, Debug)]
pub struct TownInfoScene {
    info: TownInfo,
    kingdom: KingdomId,
    keep: ScenePost,
    guard_posts: Vec<ScenePost>,
    archer_posts: Vec<ScenePost>,
    civilian_homes: Vec<ScenePost>,
    workstations: Vec<ScenePost>,
    rally_points: Vec<ScenePost>,
    raid_camps: Vec<ScenePost>,
    ram_posts: Vec<ScenePost>,
    ladder_posts: Vec<ScenePost>,
    trebuchet_posts: Vec<ScenePost>,
    road_approach: Option<ScenePost>,
}

impl TownInfoScene {
    #[must_use]
    pub fn from_info(info: TownInfo) -> Self {
        let kingdom = KingdomId::for_town(&info);
        let keep = first_role(&info, SpawnRole::RallyKeep)
            .unwrap_or_else(|| ScenePost::from_tuple(info.castle.keep.center()));
        let mut guard_posts = posts(&info, SpawnRole::Gate);
        if guard_posts.len() < 4 {
            guard_posts.extend(posts(&info, SpawnRole::GuardPost));
        }
        let mut archer_posts = posts(&info, SpawnRole::GuardPost);
        if archer_posts.is_empty() {
            archer_posts.clone_from(&guard_posts);
        }
        let mut civilian_homes = posts(&info, SpawnRole::VillagerBed);
        if civilian_homes.is_empty() {
            civilian_homes = posts(&info, SpawnRole::Workstation);
        }
        if civilian_homes.is_empty() {
            civilian_homes.push(ScenePost::from_tuple(info.market.center()));
        }
        let mut workstations = posts(&info, SpawnRole::Workstation);
        if workstations.is_empty() {
            workstations.push(ScenePost::from_tuple(info.market.center()));
        }
        let mut rally_points = posts(&info, SpawnRole::RallyKeep);
        rally_points.extend(posts(&info, SpawnRole::RallyMarket));
        rally_points.extend(posts(&info, SpawnRole::Gate));
        if rally_points.is_empty() {
            rally_points.push(keep);
        }
        let mut raid_camps = posts(&info, SpawnRole::PillagerSpawn);
        if raid_camps.is_empty() {
            raid_camps = posts(&info, SpawnRole::PillagerRally);
        }
        let ram_posts = posts(&info, SpawnRole::SiegeRam);
        let ladder_posts = posts(&info, SpawnRole::SiegeLadder);
        let trebuchet_posts = posts(&info, SpawnRole::SiegeTreb);
        let road_approach =
            first_role(&info, SpawnRole::PillagerRally).or_else(|| raid_camps.first().copied());

        Self {
            info,
            kingdom,
            keep,
            guard_posts,
            archer_posts,
            civilian_homes,
            workstations,
            rally_points,
            raid_camps,
            ram_posts,
            ladder_posts,
            trebuchet_posts,
            road_approach,
        }
    }

    #[must_use]
    pub fn info(&self) -> &TownInfo {
        &self.info
    }
}

impl TownScene for TownInfoScene {
    fn town_id(&self) -> &str {
        &self.info.id
    }

    fn kingdom(&self) -> KingdomId {
        self.kingdom.clone()
    }

    fn bounds(&self) -> Rect {
        self.info.bounds
    }

    fn keep(&self) -> ScenePost {
        self.keep
    }

    fn capture_point(&self) -> ScenePost {
        first_role(&self.info, SpawnRole::RallyMarket)
            .unwrap_or_else(|| ScenePost::from_xz(self.info.capture_point))
    }

    fn gates(&self) -> &[GateInfo] {
        &self.info.gates
    }

    fn guard_posts(&self) -> &[ScenePost] {
        &self.guard_posts
    }

    fn archer_posts(&self) -> &[ScenePost] {
        &self.archer_posts
    }

    fn civilian_homes(&self) -> &[ScenePost] {
        &self.civilian_homes
    }

    fn workstations(&self) -> &[ScenePost] {
        &self.workstations
    }

    fn rally_points(&self) -> &[ScenePost] {
        &self.rally_points
    }

    fn raid_camps(&self) -> &[ScenePost] {
        &self.raid_camps
    }

    fn ram_posts(&self) -> &[ScenePost] {
        &self.ram_posts
    }

    fn ladder_posts(&self) -> &[ScenePost] {
        &self.ladder_posts
    }

    fn trebuchet_posts(&self) -> &[ScenePost] {
        &self.trebuchet_posts
    }

    fn road_approach(&self) -> Option<ScenePost> {
        self.road_approach
    }

    fn wall(&self) -> &[[i32; 2]] {
        &self.info.town_wall
    }

    fn keep_rect(&self) -> Rect {
        self.info.castle.inner
    }

    fn gatehouse_rect(&self) -> Rect {
        self.info.castle.gatehouse
    }
}

fn posts(info: &TownInfo, role: SpawnRole) -> Vec<ScenePost> {
    info.spawns
        .iter()
        .copied()
        .filter(|s| s.role == role)
        .map(ScenePost::from_spawn)
        .collect()
}

fn first_role(info: &TownInfo, role: SpawnRole) -> Option<ScenePost> {
    info.spawns
        .iter()
        .copied()
        .find(|s| s.role == role)
        .map(ScenePost::from_spawn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;
    use crate::palette::PaletteKind;
    use crate::plan::build_plan;
    use crate::town::town_in_cell;

    fn scene() -> TownInfoScene {
        let origin = town_in_cell(3, 0, 0);
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
        let plan = build_plan(3, origin, grid, PaletteKind::Oak);
        TownInfoScene::from_info(TownInfo::from_plan(&plan))
    }

    #[test]
    fn derives_posts_from_towninfo_spawns() {
        let scene = scene();
        assert!(!scene.gates().is_empty());
        assert!(!scene.guard_posts().is_empty());
        assert!(!scene.archer_posts().is_empty());
        assert!(!scene.civilian_homes().is_empty());
        assert!(
            !scene.raid_camps().is_empty(),
            "expected pillager camp spawns on the hinterland plan"
        );
        assert!(
            scene
                .info()
                .spawns
                .iter()
                .any(|s| s.role == SpawnRole::PillagerSpawn || s.role == SpawnRole::PillagerRally)
        );
        assert!(
            !scene.ram_posts().is_empty()
                && !scene.ladder_posts().is_empty()
                && !scene.trebuchet_posts().is_empty(),
            "camp siege yard should expose ram, ladder, and trebuchet posts"
        );
        assert_eq!(scene.kingdom().as_str(), "alder");
        let keep = scene.keep();
        let gate = scene.gates()[0];
        let rally = scene.threatened_rally(gate.x + 40, gate.z);
        assert!(dist2(rally.x, rally.z, gate.x, gate.z) < dist2(rally.x, rally.z, keep.x, keep.z));
    }

    #[test]
    fn capture_zone_excludes_the_gatehouse_on_a_plan() {
        let scene = scene();
        let inner = scene.keep_rect();
        let gatehouse = scene.gatehouse_rect();
        assert!(gatehouse.width() > 0 && gatehouse.depth() > 0);
        let (gx, gz) = gatehouse.center();
        assert!(!crate::siege::in_capture_zone(
            inner,
            gatehouse,
            f64::from(gx) + 0.5,
            f64::from(gz) + 0.5
        ));
        let mut found = false;
        'search: for x in inner.min_x..=inner.max_x {
            for z in inner.min_z..=inner.max_z {
                if gatehouse.contains(x, z) {
                    continue;
                }
                assert!(crate::siege::in_capture_zone(
                    inner,
                    gatehouse,
                    f64::from(x) + 0.5,
                    f64::from(z) + 0.5
                ));
                found = true;
                break 'search;
            }
        }
        assert!(
            found,
            "inner bailey should have a capture cell outside the gatehouse"
        );
    }
}
