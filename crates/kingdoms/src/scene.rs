//! Town gameplay points derived from [`TownInfo`].
//!
//! A later agent may add villager beds, workstations, guard posts, rally
//! points and pillager camps to `TownInfo`. Until then this trait is the
//! swap point: [`TownInfoScene`] derives posts from keep, gates, towers,
//! wall polygon and lots.

use crate::buildings::BuildingKind;
use crate::faction::KingdomId;
use crate::geom::{Rect, dist2};
use crate::info::{GateInfo, GateKind, TownInfo};

/// A column a defender or civilian should care about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScenePost {
    pub x: i32,
    pub z: i32,
}

impl ScenePost {
    #[must_use]
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    #[must_use]
    pub const fn from_tuple(p: (i32, i32)) -> Self {
        Self { x: p.0, z: p.1 }
    }

    #[must_use]
    pub const fn from_xz(p: [i32; 2]) -> Self {
        Self { x: p[0], z: p[1] }
    }

    #[must_use]
    pub const fn as_tuple(self) -> (i32, i32) {
        (self.x, self.z)
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
    fn rally_points(&self) -> &[ScenePost];
    fn raid_camps(&self) -> &[ScenePost];
    fn road_approach(&self) -> Option<ScenePost>;
    fn wall(&self) -> &[[i32; 2]];

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

/// Adapter over today's planner snapshot.
#[derive(Clone, Debug)]
pub struct TownInfoScene {
    info: TownInfo,
    kingdom: KingdomId,
    guard_posts: Vec<ScenePost>,
    archer_posts: Vec<ScenePost>,
    civilian_homes: Vec<ScenePost>,
    rally_points: Vec<ScenePost>,
    raid_camps: Vec<ScenePost>,
    road_approach: Option<ScenePost>,
}

impl TownInfoScene {
    #[must_use]
    pub fn from_info(info: TownInfo) -> Self {
        let kingdom = KingdomId::for_town(&info);
        let mut guard_posts = Vec::new();
        let mut archer_posts = Vec::new();
        let mut civilian_homes = Vec::new();
        let mut rally_points = Vec::new();

        for gate in &info.gates {
            guard_posts.push(ScenePost::new(gate.x, gate.z));
            // Stand just inside the gate so patrols do not wander into the field.
            let inner =
                ScenePost::new(gate.x - gate.facing.dx() * 4, gate.z - gate.facing.dz() * 4);
            rally_points.push(inner);
        }

        for tower in &info.towers {
            archer_posts.push(ScenePost::from_tuple(tower.center()));
        }
        for tower in &info.castle.towers {
            archer_posts.push(ScenePost::from_tuple(tower.center()));
        }
        // Wall walk samples so archers have more than tower tops.
        if info.town_wall.len() >= 4 {
            let step = (info.town_wall.len() / 6).max(1);
            for (i, xz) in info.town_wall.iter().enumerate() {
                if i % step == 0 {
                    archer_posts.push(ScenePost::from_xz(*xz));
                }
            }
        }

        let barracks = info.castle.barracks.center();
        guard_posts.push(ScenePost::from_tuple(barracks));
        rally_points.push(ScenePost::from_tuple(info.castle.gatehouse.center()));
        rally_points.push(ScenePost::from_xz(info.capture_point));

        for b in &info.buildings {
            match b.kind {
                BuildingKind::Cottage
                | BuildingKind::Timber
                | BuildingKind::Townhouse
                | BuildingKind::Longhouse
                | BuildingKind::Tavern => {
                    civilian_homes.push(ScenePost::from_xz(b.center));
                }
                BuildingKind::Church => {
                    civilian_homes.push(ScenePost::from_xz(b.center));
                    rally_points.push(ScenePost::from_xz(b.center));
                }
                BuildingKind::Blacksmith | BuildingKind::Barn | BuildingKind::Windmill => {
                    civilian_homes.push(ScenePost::from_xz(b.center));
                }
                BuildingKind::Stall => {}
            }
        }
        if civilian_homes.is_empty() {
            civilian_homes.push(ScenePost::from_tuple(info.market.center()));
        }

        let road_approach = approach_from_roads(&info);
        let raid_camps = raid_camps_from(&info, road_approach);

        Self {
            info,
            kingdom,
            guard_posts,
            archer_posts,
            civilian_homes,
            rally_points,
            raid_camps,
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
        ScenePost::from_tuple(self.info.castle.keep.center())
    }

    fn capture_point(&self) -> ScenePost {
        ScenePost::from_xz(self.info.capture_point)
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

    fn rally_points(&self) -> &[ScenePost] {
        &self.rally_points
    }

    fn raid_camps(&self) -> &[ScenePost] {
        &self.raid_camps
    }

    fn road_approach(&self) -> Option<ScenePost> {
        self.road_approach
    }

    fn wall(&self) -> &[[i32; 2]] {
        &self.info.town_wall
    }
}

fn approach_from_roads(info: &TownInfo) -> Option<ScenePost> {
    let town_gates: Vec<_> = info
        .gates
        .iter()
        .filter(|g| g.kind == GateKind::Town)
        .copied()
        .collect();
    let gate = town_gates
        .first()
        .copied()
        .or_else(|| info.gates.first().copied())?;
    // Prefer a road endpoint that is farther from the keep than the gate — that is the field.
    let keep = info.castle.keep.center();
    let mut best: Option<(i64, ScenePost)> = None;
    for road in &info.roads {
        for end in [road.from, road.to] {
            let d_keep = dist2(end[0], end[1], keep.0, keep.1);
            let d_gate = dist2(end[0], end[1], gate.x, gate.z);
            if d_keep > d_gate {
                match best {
                    Some((best_d, _)) if d_keep <= best_d => {}
                    _ => best = Some((d_keep, ScenePost::from_xz(end))),
                }
            }
        }
    }
    best.map(|(_, p)| p).or_else(|| {
        Some(ScenePost::new(
            gate.x + gate.facing.dx() * 80,
            gate.z + gate.facing.dz() * 80,
        ))
    })
}

fn raid_camps_from(info: &TownInfo, approach: Option<ScenePost>) -> Vec<ScenePost> {
    let mut camps = Vec::new();
    if let Some(p) = approach {
        camps.push(p);
    }
    for gate in info.gates.iter().filter(|g| g.kind == GateKind::Town) {
        camps.push(ScenePost::new(
            gate.x + gate.facing.dx() * 96,
            gate.z + gate.facing.dz() * 96,
        ));
    }
    if camps.is_empty() {
        let (cx, cz) = info.bounds.center();
        camps.push(ScenePost::new(cx, cz + 120));
    }
    camps
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
    fn derives_posts_from_existing_towninfo() {
        let scene = scene();
        assert!(!scene.gates().is_empty());
        assert!(!scene.guard_posts().is_empty());
        assert!(!scene.archer_posts().is_empty());
        assert!(!scene.civilian_homes().is_empty());
        assert!(!scene.raid_camps().is_empty());
        assert_eq!(scene.kingdom().as_str(), "alder");
        let keep = scene.keep();
        let gate = scene.gates()[0];
        let rally = scene.threatened_rally(gate.x + 40, gate.z);
        assert!(dist2(rally.x, rally.z, gate.x, gate.z) < dist2(rally.x, rally.z, keep.x, keep.z));
    }
}
