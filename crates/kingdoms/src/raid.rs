//! Pillager war bands that march on a town.
//!
//! This is a kingdoms raid, not vanilla's omen raid. Waves spawn at a camp
//! outside the walls, path toward the threatened gate, and include a banner
//! captain. An HTTP director can call [`crate::director::start_raid`] later.

use pumpkin_data::entity::EntityType;
use pumpkin_util::math::vector3::Vector3;

use crate::scene::ScenePost;

/// How to size a raid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RaidSpec {
    pub raiders: u16,
    /// Extra town defenders to spawn for large sieges (0 = standing garrison only).
    pub extra_defenders: u16,
    pub waves: u8,
}

impl RaidSpec {
    #[must_use]
    pub fn new(raiders: u16) -> Self {
        let waves = u32::from(raiders).div_ceil(40).clamp(1, 8) as u8;
        Self {
            raiders,
            extra_defenders: 0,
            waves,
        }
    }

    #[must_use]
    pub fn with_extra_defenders(mut self, extra: u16) -> Self {
        self.extra_defenders = extra;
        self
    }

    #[must_use]
    pub fn wave_count(self) -> usize {
        usize::from(self.waves.max(1))
    }

    /// Raiders in `wave` (0-based). Remainder goes to the last wave.
    #[must_use]
    pub fn raiders_in_wave(self, wave: u8) -> u16 {
        let waves = u16::from(self.waves.max(1));
        let base = self.raiders / waves;
        let rem = self.raiders % waves;
        if u16::from(wave) + 1 == waves {
            base + rem
        } else {
            base
        }
    }
}

/// One mob in a wave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaveMember {
    pub entity_type: &'static EntityType,
    pub captain: bool,
}

/// Mix of crossbow pillagers and melee vindicators, with a captain on the
/// first member of each wave.
#[must_use]
pub fn compose_wave(count: u16, _wave: u8) -> Vec<WaveMember> {
    let mut members = Vec::with_capacity(count as usize);
    for i in 0..count {
        let captain = i == 0;
        let entity_type = if i % 5 == 4 {
            &EntityType::VINDICATOR
        } else {
            &EntityType::PILLAGER
        };
        members.push(WaveMember {
            entity_type,
            captain,
        });
    }
    members
}

/// Spread spawn columns around a camp so 200 mobs are not in one block.
#[must_use]
pub fn spawn_offset(camp: ScenePost, index: u16, y: i32) -> Vector3<f64> {
    let ring = i32::from(index / 8);
    let slot = i32::from(index % 8);
    let ang = slot as f64 * std::f64::consts::FRAC_PI_4;
    let r = f64::from(3 + ring * 2);
    Vector3::new(
        f64::from(camp.x) + 0.5 + ang.cos() * r,
        f64::from(y) + 1.0,
        f64::from(camp.z) + 0.5 + ang.sin() * r,
    )
}

/// Ticks between waves after the previous wave is mostly dead, or this long
/// after the previous spawn, whichever comes first.
pub const WAVE_SPACING_TICKS: u32 = 200;
pub const WAVE_ADVANCE_REMAINING: u16 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waves_cover_the_requested_size() {
        let spec = RaidSpec::new(200);
        assert_eq!(spec.waves, 5);
        let total: u16 = (0..spec.waves).map(|w| spec.raiders_in_wave(w)).sum();
        assert_eq!(total, 200);
        let last = compose_wave(spec.raiders_in_wave(0), 0);
        assert!(last[0].captain);
        assert_eq!(last.iter().filter(|m| m.captain).count(), 1);
        assert!(
            last.iter()
                .any(|m| m.entity_type == &EntityType::VINDICATOR)
        );
    }

    #[test]
    fn small_raid_is_one_wave() {
        let spec = RaidSpec::new(12);
        assert_eq!(spec.waves, 1);
        assert_eq!(spec.raiders_in_wave(0), 12);
    }
}
