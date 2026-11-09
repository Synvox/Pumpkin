//! Villager combat roles and how many of each a town should field.

use crate::scene::{ScenePost, TownScene};

/// What a town villager does when the alarm sounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VillagerRole {
    /// Patrol walls, gates and towers; melee pillagers with sword or axe.
    Guard,
    /// Hold a tower or wall walk; shoot with bow or crossbow.
    Archer,
    /// Flee to the keep or a house, ring the bell, return after.
    Civilian,
    /// Hook for later repair-by-diff using `plan.planned_block`. Idle for now.
    Builder,
}

impl VillagerRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Guard => "guard",
            Self::Archer => "archer",
            Self::Civilian => "civilian",
            Self::Builder => "builder",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "guard" => Some(Self::Guard),
            "archer" => Some(Self::Archer),
            "civilian" => Some(Self::Civilian),
            "builder" => Some(Self::Builder),
            _ => None,
        }
    }

    /// Fights on the wall instead of hiding.
    #[must_use]
    pub const fn is_combatant(self) -> bool {
        matches!(self, Self::Guard | Self::Archer)
    }
}

/// One planned citizen. Index is stable for a given town so respawn can reuse it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CitizenSlot {
    pub index: u16,
    pub role: VillagerRole,
    pub home: ScenePost,
    pub is_golem: bool,
}

/// Standing population derived from the scene. Small by default; raids can add
/// extra defenders through [`PopulationSpec`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PopulationSpec {
    pub guards: u16,
    pub archers: u16,
    pub civilians: u16,
    pub builders: u16,
    pub golems: u16,
}

impl PopulationSpec {
    /// Default garrison sized from the town's posts, capped so a quiet town
    /// does not spawn hundreds of villagers.
    #[must_use]
    pub fn from_scene(scene: &dyn TownScene) -> Self {
        let guards = scene.guard_posts().len().clamp(4, 16) as u16;
        let archers = scene.archer_posts().len().clamp(4, 12) as u16;
        let civilians = scene.civilian_homes().len().clamp(8, 24) as u16;
        Self {
            guards,
            archers,
            civilians,
            builders: 1,
            golems: 2,
        }
    }

    /// Scale up for a head-to-head siege. Guards/archers take the extra count.
    #[must_use]
    pub fn with_defenders(mut self, n: u16) -> Self {
        if n == 0 {
            return self;
        }
        let combat = self.guards.saturating_add(self.archers).max(1);
        let extra = n.saturating_sub(self.total());
        if extra == 0 {
            return self;
        }
        let extra_guards = extra * self.guards / combat;
        let extra_archers = extra.saturating_sub(extra_guards);
        self.guards = self.guards.saturating_add(extra_guards);
        self.archers = self.archers.saturating_add(extra_archers);
        self
    }

    #[must_use]
    pub const fn total(self) -> u16 {
        self.guards
            .saturating_add(self.archers)
            .saturating_add(self.civilians)
            .saturating_add(self.builders)
            .saturating_add(self.golems)
    }
}

/// Deterministic citizen slots for a town.
#[must_use]
pub fn citizen_slots(scene: &dyn TownScene, spec: PopulationSpec) -> Vec<CitizenSlot> {
    let mut slots = Vec::with_capacity(spec.total() as usize);
    let mut index = 0u16;
    push_role(
        &mut slots,
        &mut index,
        VillagerRole::Guard,
        spec.guards,
        scene.guard_posts(),
        scene.keep(),
        false,
    );
    push_role(
        &mut slots,
        &mut index,
        VillagerRole::Archer,
        spec.archers,
        scene.archer_posts(),
        scene.keep(),
        false,
    );
    push_role(
        &mut slots,
        &mut index,
        VillagerRole::Civilian,
        spec.civilians,
        scene.civilian_homes(),
        scene.keep(),
        false,
    );
    push_role(
        &mut slots,
        &mut index,
        VillagerRole::Builder,
        spec.builders,
        scene.rally_points(),
        scene.keep(),
        false,
    );
    let golem_homes = scene.rally_points();
    for i in 0..spec.golems {
        let home = pick_post(golem_homes, i, scene.keep());
        slots.push(CitizenSlot {
            index,
            role: VillagerRole::Guard,
            home,
            is_golem: true,
        });
        index = index.saturating_add(1);
    }
    slots
}

fn push_role(
    slots: &mut Vec<CitizenSlot>,
    index: &mut u16,
    role: VillagerRole,
    count: u16,
    posts: &[ScenePost],
    fallback: ScenePost,
    is_golem: bool,
) {
    for i in 0..count {
        slots.push(CitizenSlot {
            index: *index,
            role,
            home: pick_post(posts, i, fallback),
            is_golem,
        });
        *index = index.saturating_add(1);
    }
}

fn pick_post(posts: &[ScenePost], i: u16, fallback: ScenePost) -> ScenePost {
    if posts.is_empty() {
        return fallback;
    }
    posts[i as usize % posts.len()]
}

/// Which goal a role should run given town alarm state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalKind {
    Patrol,
    HoldPost,
    Melee,
    Ranged,
    FleeKeep,
    RingBell,
    ReturnHome,
    BuilderStub,
}

/// Picks the high-level behaviour. Combat targeting is separate.
#[must_use]
pub fn select_goal(role: VillagerRole, alarm: bool, in_melee_range: bool) -> GoalKind {
    match role {
        VillagerRole::Guard => {
            if in_melee_range {
                GoalKind::Melee
            } else {
                GoalKind::Patrol
            }
        }
        VillagerRole::Archer => {
            if alarm {
                GoalKind::Ranged
            } else {
                GoalKind::HoldPost
            }
        }
        VillagerRole::Civilian => {
            if alarm {
                GoalKind::FleeKeep
            } else {
                GoalKind::ReturnHome
            }
        }
        VillagerRole::Builder => GoalKind::BuilderStub,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;
    use crate::info::TownInfo;
    use crate::palette::PaletteKind;
    use crate::plan::build_plan;
    use crate::scene::TownInfoScene;
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
    fn default_population_is_stable_and_uses_posts() {
        let scene = scene();
        let spec = PopulationSpec::from_scene(&scene);
        let slots = citizen_slots(&scene, spec);
        assert_eq!(slots.len(), spec.total() as usize);
        assert!(
            slots
                .iter()
                .any(|s| s.role == VillagerRole::Guard && !s.is_golem)
        );
        assert!(slots.iter().any(|s| s.role == VillagerRole::Archer));
        assert!(slots.iter().any(|s| s.role == VillagerRole::Civilian));
        assert!(slots.iter().any(|s| s.role == VillagerRole::Builder));
        assert_eq!(
            slots.iter().filter(|s| s.is_golem).count(),
            spec.golems as usize
        );
        let again = citizen_slots(&scene, spec);
        assert_eq!(slots, again);
    }

    #[test]
    fn alarm_sends_civilians_to_keep_and_guards_to_fight() {
        assert_eq!(
            select_goal(VillagerRole::Civilian, true, false),
            GoalKind::FleeKeep
        );
        assert_eq!(
            select_goal(VillagerRole::Civilian, false, false),
            GoalKind::ReturnHome
        );
        assert_eq!(
            select_goal(VillagerRole::Guard, true, true),
            GoalKind::Melee
        );
        assert_eq!(
            select_goal(VillagerRole::Archer, true, false),
            GoalKind::Ranged
        );
        assert_eq!(
            select_goal(VillagerRole::Builder, true, false),
            GoalKind::BuilderStub
        );
    }

    #[test]
    fn with_defenders_scales_combat_roles() {
        let spec = PopulationSpec {
            guards: 8,
            archers: 8,
            civilians: 10,
            builders: 1,
            golems: 2,
        };
        let scaled = spec.with_defenders(200);
        assert!(scaled.total() >= 200);
        assert!(scaled.guards + scaled.archers > spec.guards + spec.archers);
    }
}
