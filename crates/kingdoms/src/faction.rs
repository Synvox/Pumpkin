//! Kingdoms, pillagers, and who may attack whom.
//!
//! Each town belongs to a kingdom. Villagers spawned there are its citizens.
//! Pillagers are a single hostile faction. Players join a kingdom by command
//! so they fight alongside its villagers with friendly fire off.

use std::fmt;
use std::sync::Arc;

use pumpkin_core::entity::EntityBase;
use pumpkin_core::entity::player::Player;
use pumpkin_data::entity::EntityType;

use crate::info::TownInfo;

/// Scoreboard tag prefix so citizens survive chunk reload.
pub const TAG_PREFIX: &str = "kingdoms";
pub const TAG_CITIZEN: &str = "kingdoms:citizen";
pub const TAG_RAIDER: &str = "kingdoms:raider";
pub const TAG_GOLEM: &str = "kingdoms:golem";
pub const TAG_CAPTAIN: &str = "kingdoms:captain";
pub const TAG_ROLE: &str = "kingdoms:role";
pub const TAG_TOWN: &str = "kingdoms:town";
pub const TAG_KINGDOM: &str = "kingdoms:kingdom";
pub const TAG_WIRED: &str = "kingdoms:wired";
pub const TAG_JOB_RAM: &str = "kingdoms:job:ram";
pub const TAG_JOB_LADDER: &str = "kingdoms:job:ladder";

/// Stable kingdom identity. Towns in even/odd cells share two starting kingdoms
/// so a later director can pit them against each other; pillagers stay outside.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KingdomId(pub Arc<str>);

impl KingdomId {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self(Arc::from(normalize_kingdom_name(name).as_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Kingdom that currently holds this town.
    #[must_use]
    pub fn for_town(info: &TownInfo) -> Self {
        Self::for_cell(info.cell[0], info.cell[1])
    }

    #[must_use]
    pub fn for_cell(cell_x: i32, cell_z: i32) -> Self {
        // Two starting kingdoms. Capture later can flip a town between them.
        if cell_x.wrapping_add(cell_z).rem_euclid(2) == 0 {
            Self::new("alder")
        } else {
            Self::new("raven")
        }
    }
}

impl fmt::Display for KingdomId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn normalize_kingdom_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '_' || ch == '-' {
            out.push(ch);
        }
    }
    if out.is_empty() {
        "alder".to_owned()
    } else {
        out
    }
}

/// Combat side used for targeting and friendly fire.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Faction {
    Kingdom(KingdomId),
    Pillager,
    Neutral,
}

impl Faction {
    #[must_use]
    pub const fn is_kingdom(&self) -> bool {
        matches!(self, Self::Kingdom(_))
    }

    #[must_use]
    pub fn kingdom_id(&self) -> Option<&KingdomId> {
        match self {
            Self::Kingdom(id) => Some(id),
            Self::Pillager | Self::Neutral => None,
        }
    }
}

/// Whether `attacker` may damage `victim`.
#[must_use]
pub fn is_hostile(attacker: &Faction, victim: &Faction) -> bool {
    match (attacker, victim) {
        (Faction::Neutral, _) | (_, Faction::Neutral) => false,
        (Faction::Pillager, Faction::Pillager) => false,
        (Faction::Pillager, Faction::Kingdom(_)) | (Faction::Kingdom(_), Faction::Pillager) => true,
        (Faction::Kingdom(a), Faction::Kingdom(b)) => a != b,
    }
}

/// Same-side: no friendly fire, no targeting.
#[must_use]
pub fn is_allied(a: &Faction, b: &Faction) -> bool {
    a == b && !matches!(a, Faction::Neutral)
}

/// Vanilla entity types that fight for the pillager side.
#[must_use]
pub fn is_pillager_type(entity_type: &'static EntityType) -> bool {
    entity_type == &EntityType::PILLAGER
        || entity_type == &EntityType::VINDICATOR
        || entity_type == &EntityType::EVOKER
        || entity_type == &EntityType::RAVAGER
        || entity_type == &EntityType::WITCH
        || entity_type == &EntityType::ILLUSIONER
        || entity_type == &EntityType::VEX
}

/// Vanilla entity types that fight for a town unless a player has joined the other side.
#[must_use]
pub fn is_defender_type(entity_type: &'static EntityType) -> bool {
    entity_type == &EntityType::VILLAGER
        || entity_type == &EntityType::IRON_GOLEM
        || entity_type == &EntityType::PLAYER
}

/// Tag used on a living entity for its role (`guard`, `archer`, …).
#[must_use]
pub fn role_tag(role: &str) -> String {
    format!("{TAG_ROLE}:{role}")
}

/// Tag used on a living entity for its town id (sanitized).
#[must_use]
pub fn town_tag(town_id: &str) -> String {
    format!("{TAG_TOWN}:{}", sanitize_tag(town_id))
}

#[must_use]
pub fn kingdom_tag(kingdom: &KingdomId) -> String {
    format!("{TAG_KINGDOM}:{}", kingdom.as_str())
}

#[must_use]
pub fn sanitize_tag(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

/// Reads kingdoms tags on an entity. Falls back to vanilla type when untagged.
#[must_use]
pub fn faction_of_entity(entity: &dyn EntityBase, player_kingdom: Option<&KingdomId>) -> Faction {
    let base = entity.get_entity();
    if let Some(player) = entity.get_player() {
        return player_kingdom
            .cloned()
            .map_or_else(|| faction_of_player(player), Faction::Kingdom);
    }
    if let Some(faction) = faction_from_tags(base) {
        return faction;
    }
    let ty = base.entity_type;
    if is_pillager_type(ty) {
        Faction::Pillager
    } else if ty == &EntityType::VILLAGER || ty == &EntityType::IRON_GOLEM {
        // Untagged villagers outside a town stay neutral so vanilla villages are untouched.
        Faction::Neutral
    } else {
        Faction::Neutral
    }
}

fn faction_of_player(player: &Player) -> Faction {
    faction_from_tags(&player.living_entity.entity).unwrap_or(Faction::Neutral)
}

/// Kingdom / pillager tags written at spawn.
pub fn faction_from_tags(entity: &pumpkin_core::entity::Entity) -> Option<Faction> {
    let tags = entity
        .scoreboard_tags
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if tags.iter().any(|t| t == TAG_RAIDER) {
        return Some(Faction::Pillager);
    }
    for tag in tags.iter() {
        if let Some(rest) = tag.strip_prefix(&format!("{TAG_KINGDOM}:")) {
            return Some(Faction::Kingdom(KingdomId::new(rest)));
        }
    }
    if tags.iter().any(|t| t == TAG_CITIZEN || t == TAG_GOLEM) {
        // Citizen without a kingdom tag: treat as allied to nobody extra.
        return Some(Faction::Neutral);
    }
    None
}

/// True when this entity carries any kingdoms gameplay tag.
#[must_use]
pub fn is_kingdoms_tagged(entity: &pumpkin_core::entity::Entity) -> bool {
    let tags = entity
        .scoreboard_tags
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tags.iter().any(|t| t.starts_with(TAG_PREFIX))
}

/// First matching `kingdoms:role:*` tag.
#[must_use]
pub fn role_from_tags(entity: &pumpkin_core::entity::Entity) -> Option<String> {
    let tags = entity
        .scoreboard_tags
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tags.iter().find_map(|t| {
        t.strip_prefix(&format!("{TAG_ROLE}:"))
            .map(std::string::ToString::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alder() -> Faction {
        Faction::Kingdom(KingdomId::new("alder"))
    }

    fn raven() -> Faction {
        Faction::Kingdom(KingdomId::new("raven"))
    }

    #[test]
    fn pillagers_are_hostile_to_kingdoms_not_each_other() {
        assert!(is_hostile(&Faction::Pillager, &alder()));
        assert!(is_hostile(&alder(), &Faction::Pillager));
        assert!(!is_hostile(&Faction::Pillager, &Faction::Pillager));
        assert!(is_allied(&Faction::Pillager, &Faction::Pillager));
    }

    #[test]
    fn same_kingdom_is_allied_different_is_hostile() {
        assert!(is_allied(&alder(), &alder()));
        assert!(!is_hostile(&alder(), &alder()));
        assert!(is_hostile(&alder(), &raven()));
        assert!(!is_allied(&alder(), &raven()));
    }

    #[test]
    fn neutrals_never_fight() {
        assert!(!is_hostile(&Faction::Neutral, &alder()));
        assert!(!is_hostile(&alder(), &Faction::Neutral));
        assert!(!is_allied(&Faction::Neutral, &Faction::Neutral));
    }

    #[test]
    fn even_odd_cells_split_two_kingdoms() {
        assert_eq!(KingdomId::for_cell(0, 0).as_str(), "alder");
        assert_eq!(KingdomId::for_cell(1, 0).as_str(), "raven");
        assert_eq!(KingdomId::for_cell(0, 1).as_str(), "raven");
    }

    #[test]
    fn kingdom_names_normalize() {
        assert_eq!(KingdomId::new("Alder").as_str(), "alder");
        assert_eq!(KingdomId::new("Raven Hold!").as_str(), "ravenhold");
    }

    #[test]
    fn pillager_types_match_vanilla_raiders() {
        assert!(is_pillager_type(&EntityType::PILLAGER));
        assert!(is_pillager_type(&EntityType::VINDICATOR));
        assert!(!is_pillager_type(&EntityType::VILLAGER));
        assert!(is_defender_type(&EntityType::IRON_GOLEM));
    }
}
