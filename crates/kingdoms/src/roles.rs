//! Villager combat roles and how many of each a town should field.
//!
//! Kits are distinct so a joined client can tell archers, spearmen, swordsmen,
//! builders, captains and standard-bearers apart from held items and armour.

use crate::scene::{ScenePost, TownScene};

/// What a town villager does when the alarm sounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VillagerRole {
    /// Patrol walls, gates and towers; melee with sword and shield.
    Guard,
    /// Hold the gate line; melee with a spear and no shield.
    Spearman,
    /// Hold a tower or wall walk; shoot with a bow.
    Archer,
    /// Flee to the keep or a house, ring the bell, return after.
    Civilian,
    /// Walks the post-siege repair queue and places missing plan blocks.
    Builder,
    /// Sells TNT for emeralds at the market / barracks.
    Quartermaster,
    /// Leads a squad; iron kit and a kingdom banner on the head.
    Captain,
    /// Carries the kingdom standard in the off hand; rallies routed soldiers.
    Bearer,
}

impl VillagerRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Guard => "guard",
            Self::Spearman => "spearman",
            Self::Archer => "archer",
            Self::Civilian => "civilian",
            Self::Builder => "builder",
            Self::Quartermaster => "quartermaster",
            Self::Captain => "captain",
            Self::Bearer => "bearer",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "guard" => Some(Self::Guard),
            "spearman" => Some(Self::Spearman),
            "archer" => Some(Self::Archer),
            "civilian" => Some(Self::Civilian),
            "builder" => Some(Self::Builder),
            "quartermaster" => Some(Self::Quartermaster),
            "captain" => Some(Self::Captain),
            "bearer" => Some(Self::Bearer),
            _ => None,
        }
    }

    /// Fights on the wall instead of hiding.
    #[must_use]
    pub const fn is_combatant(self) -> bool {
        matches!(
            self,
            Self::Guard | Self::Spearman | Self::Archer | Self::Captain | Self::Bearer
        )
    }

    /// Main-hand item a client can use to tell this role apart.
    #[must_use]
    pub const fn held_item_key(self) -> &'static str {
        match self {
            Self::Guard | Self::Captain | Self::Bearer => "iron_sword",
            Self::Spearman => "iron_spear",
            Self::Archer => "bow",
            Self::Builder => "iron_axe",
            Self::Quartermaster => "emerald",
            Self::Civilian => "",
        }
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
    pub spearmen: u16,
    pub archers: u16,
    pub civilians: u16,
    pub builders: u16,
    pub quartermasters: u16,
    pub captains: u16,
    pub bearers: u16,
    pub golems: u16,
}

impl PopulationSpec {
    /// Default garrison sized from the town's posts, capped so a quiet town
    /// does not spawn hundreds of villagers.
    #[must_use]
    pub fn from_scene(scene: &dyn TownScene) -> Self {
        let posts = scene.guard_posts().len().clamp(4, 16) as u16;
        let spearmen = (posts / 3).clamp(2, 6);
        let guards = posts.saturating_sub(spearmen).max(2);
        let archers = scene.archer_posts().len().clamp(4, 12) as u16;
        let civilians = scene.civilian_homes().len().clamp(8, 24) as u16;
        Self {
            guards,
            spearmen,
            archers,
            civilians,
            builders: 4,
            quartermasters: 1,
            captains: 1,
            bearers: 1,
            golems: 2,
        }
    }

    /// Scale up for a head-to-head siege. Combat roles take the extra count.
    #[must_use]
    pub fn with_defenders(mut self, n: u16) -> Self {
        if n == 0 {
            return self;
        }
        let combat = self
            .guards
            .saturating_add(self.spearmen)
            .saturating_add(self.archers)
            .max(1);
        let extra = n.saturating_sub(self.total());
        if extra == 0 {
            return self;
        }
        let extra_guards = extra * self.guards / combat;
        let extra_spearmen = extra * self.spearmen / combat;
        let extra_archers = extra
            .saturating_sub(extra_guards)
            .saturating_sub(extra_spearmen);
        self.guards = self.guards.saturating_add(extra_guards);
        self.spearmen = self.spearmen.saturating_add(extra_spearmen);
        self.archers = self.archers.saturating_add(extra_archers);
        self.captains = self.captains.max(1);
        self.bearers = self.bearers.max(1);
        self
    }

    #[must_use]
    pub const fn total(self) -> u16 {
        self.guards
            .saturating_add(self.spearmen)
            .saturating_add(self.archers)
            .saturating_add(self.civilians)
            .saturating_add(self.builders)
            .saturating_add(self.quartermasters)
            .saturating_add(self.captains)
            .saturating_add(self.bearers)
            .saturating_add(self.golems)
    }
}

/// Combat mix for extra defenders and kingdom-army waves. Index 0 is a captain
/// so a small batch still fields a recognisable leader.
#[must_use]
pub const fn wave_combat_role(index: u16) -> VillagerRole {
    match index % 12 {
        0 => VillagerRole::Captain,
        1 => VillagerRole::Bearer,
        2 | 6 | 10 => VillagerRole::Archer,
        3 | 7 => VillagerRole::Spearman,
        11 => VillagerRole::Builder,
        _ => VillagerRole::Guard,
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
        VillagerRole::Spearman,
        spec.spearmen,
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
    push_role(
        &mut slots,
        &mut index,
        VillagerRole::Quartermaster,
        spec.quartermasters,
        scene.rally_points(),
        scene.keep(),
        false,
    );
    push_role(
        &mut slots,
        &mut index,
        VillagerRole::Captain,
        spec.captains,
        scene.guard_posts(),
        scene.keep(),
        false,
    );
    push_role(
        &mut slots,
        &mut index,
        VillagerRole::Bearer,
        spec.bearers,
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
        VillagerRole::Guard | VillagerRole::Spearman | VillagerRole::Captain => {
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
        VillagerRole::Bearer => {
            if in_melee_range {
                GoalKind::Melee
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
        VillagerRole::Quartermaster => {
            if alarm {
                GoalKind::FleeKeep
            } else {
                GoalKind::ReturnHome
            }
        }
    }
}

/// Client-visible class from equipment registry keys (`minecraft:iron_spear`).
#[must_use]
pub fn classify_equipment(keys: &[&str]) -> Option<&'static str> {
    let has = |part: &str| {
        keys.iter()
            .any(|k| k.rsplit_once(':').map_or(*k, |(_, n)| n).contains(part))
    };
    if has("banner") {
        let iron_body = has("iron_chestplate") || has("iron_leggings") || has("iron_boots");
        if iron_body {
            return Some("captain");
        }
        return Some("bearer");
    }
    if has("spear") {
        return Some("spearman");
    }
    if has("crossbow") {
        return Some("crossbowman");
    }
    if has("bow") {
        return Some("archer");
    }
    if has("pickaxe") {
        return Some("ram");
    }
    if has("flint") || has("tnt") {
        return Some("sapper");
    }
    if has("axe") && has("shield") {
        return Some("melee");
    }
    if has("axe") {
        return Some("builder");
    }
    if has("sword") {
        return Some("swordsman");
    }
    None
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
        assert!(slots.iter().any(|s| s.role == VillagerRole::Spearman));
        assert!(slots.iter().any(|s| s.role == VillagerRole::Archer));
        assert!(slots.iter().any(|s| s.role == VillagerRole::Civilian));
        assert!(slots.iter().any(|s| s.role == VillagerRole::Builder));
        assert!(slots.iter().any(|s| s.role == VillagerRole::Quartermaster));
        assert!(slots.iter().any(|s| s.role == VillagerRole::Captain));
        assert!(slots.iter().any(|s| s.role == VillagerRole::Bearer));
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
            select_goal(VillagerRole::Spearman, true, true),
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
        assert_eq!(
            select_goal(VillagerRole::Captain, true, true),
            GoalKind::Melee
        );
        assert_eq!(
            select_goal(VillagerRole::Bearer, true, false),
            GoalKind::HoldPost
        );
        assert_eq!(
            select_goal(VillagerRole::Quartermaster, true, false),
            GoalKind::FleeKeep
        );
        assert_eq!(
            select_goal(VillagerRole::Quartermaster, false, false),
            GoalKind::ReturnHome
        );
    }

    #[test]
    fn with_defenders_scales_combat_roles() {
        let spec = PopulationSpec {
            guards: 8,
            spearmen: 4,
            archers: 8,
            civilians: 10,
            builders: 1,
            quartermasters: 1,
            captains: 1,
            bearers: 1,
            golems: 2,
        };
        let scaled = spec.with_defenders(200);
        assert!(scaled.total() >= 200);
        assert!(scaled.guards + scaled.spearmen + scaled.archers > spec.guards + spec.archers);
        assert!(scaled.captains >= 1);
        assert!(scaled.bearers >= 1);
    }

    #[test]
    fn wave_mix_fields_every_visible_role() {
        let roles: Vec<_> = (0..12).map(wave_combat_role).collect();
        assert_eq!(roles[0], VillagerRole::Captain);
        assert_eq!(roles[1], VillagerRole::Bearer);
        assert!(roles.contains(&VillagerRole::Archer));
        assert!(roles.contains(&VillagerRole::Spearman));
        assert!(roles.contains(&VillagerRole::Guard));
        assert!(roles.contains(&VillagerRole::Builder));
    }

    #[test]
    fn equipment_keys_tell_roles_apart() {
        assert_eq!(
            classify_equipment(&["minecraft:iron_spear"]),
            Some("spearman")
        );
        assert_eq!(classify_equipment(&["minecraft:bow"]), Some("archer"));
        assert_eq!(
            classify_equipment(&["minecraft:crossbow"]),
            Some("crossbowman")
        );
        assert_eq!(
            classify_equipment(&["minecraft:iron_sword", "minecraft:shield"]),
            Some("swordsman")
        );
        assert_eq!(classify_equipment(&["minecraft:iron_axe"]), Some("builder"));
        assert_eq!(
            classify_equipment(&[
                "minecraft:green_banner",
                "minecraft:iron_sword",
                "minecraft:iron_chestplate",
                "minecraft:iron_leggings",
                "minecraft:iron_boots",
            ]),
            Some("captain")
        );
        assert_eq!(
            classify_equipment(&[
                "minecraft:green_banner",
                "minecraft:iron_sword",
                "minecraft:leather_chestplate",
            ]),
            Some("bearer")
        );
        assert_eq!(
            classify_equipment(&["minecraft:white_banner"]),
            Some("bearer")
        );
        let names: Vec<&str> = [
            VillagerRole::Guard,
            VillagerRole::Spearman,
            VillagerRole::Archer,
            VillagerRole::Builder,
            VillagerRole::Captain,
            VillagerRole::Bearer,
        ]
        .into_iter()
        .map(VillagerRole::held_item_key)
        .collect();
        assert!(names.contains(&"iron_spear"));
        assert!(names.contains(&"bow"));
        assert!(names.contains(&"iron_axe"));
        assert!(names.contains(&"iron_sword"));
    }
}
