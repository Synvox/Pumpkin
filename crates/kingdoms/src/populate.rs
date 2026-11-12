//! Spawn town citizens when a chunk is first generated.
//!
//! Home columns are a pure function of the plan, so each villager appears in
//! exactly one chunk. Reloaded chunks keep their entities; empty slots are
//! filled again by [`respawn_missing`].

use std::sync::Arc;

use pumpkin_core::plugin::api::events::world::ChunkPopulateEvent;
use pumpkin_core::plugin::{EventHandler, PluginFuture};
use pumpkin_core::server::Server;
use pumpkin_core::world::World;
use pumpkin_data::dimension::Dimension;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;
use tracing::debug;

use crate::director;
use crate::info::TownInfo;
use crate::roles::{PopulationSpec, citizen_slots};
use crate::scene::{TownInfoScene, TownScene};
use crate::town::towns_touching_chunk;

pub struct PopulateCitizens;

impl EventHandler<ChunkPopulateEvent> for PopulateCitizens {
    fn handle_blocking<'a>(
        &'a self,
        _server: &'a Arc<Server>,
        event: &'a mut ChunkPopulateEvent,
    ) -> PluginFuture<'a, ()> {
        let chunk = event.chunk_pos;
        Box::pin(async move {
            let worlds = director::lock().attached_worlds();
            for world in worlds {
                populate_chunk(&world, chunk.x, chunk.y);
            }
        })
    }
}

fn populate_chunk(world: &Arc<World>, chunk_x: i32, chunk_z: i32) {
    if world.dimension.minecraft_name != Dimension::OVERWORLD.minecraft_name {
        return;
    }
    let seed = world.level.seed.0;
    for town in towns_touching_chunk(seed, chunk_x, chunk_z) {
        let Some(plan) = crate::plan::cached_plan(seed, town.cell_x, town.cell_z) else {
            crate::plan::request_plan(seed, town.cell_x, town.cell_z);
            continue;
        };
        let info = TownInfo::from_plan(&plan);
        let scene = TownInfoScene::from_info(info);
        director::lock().ensure_town_from_info(scene.info().clone());
        let spec = PopulationSpec::from_scene(&scene);
        let slots = citizen_slots(&scene, spec);
        let kingdom = scene.kingdom();
        let town_id = scene.town_id().to_owned();
        let mut spawned = 0u32;
        for slot in slots {
            if slot.home.x >> 4 != chunk_x || slot.home.z >> 4 != chunk_z {
                continue;
            }
            let y = slot.home.height_or(
                world
                    .sample_surface_height(slot.home.x, slot.home.z)
                    .unwrap_or(world.sea_level)
                    + 1,
            );
            let pos = Vector3::new(
                f64::from(slot.home.x) + 0.5,
                f64::from(y),
                f64::from(slot.home.z) + 0.5,
            );
            if director::spawn_citizen(world, &town_id, &kingdom, slot.role, slot.is_golem, pos)
                .is_some()
            {
                spawned += 1;
            }
        }
        if spawned > 0 {
            debug!(
                town = %town_id,
                chunk_x,
                chunk_z,
                spawned,
                "populated town citizens"
            );
        }
    }
}

/// Max bodies to spawn on one director tick so a wipe does not hitch.
const RESPAWN_PER_TICK: usize = 2;

/// Fill empty citizen slots if the home chunk is loaded.
///
/// Deterministic homes mean a killed guard comes back at the same post.
/// Skipped while a raid is alarming the town so capture holds are not
/// restocked mid-fight.
pub fn respawn_missing(world: &Arc<World>, town_id: &str) {
    if director::town_is_fighting(town_id) {
        return;
    }
    let seed = world.level.seed.0;
    let Some(info) = crate::info::town_info_from_id(seed, town_id) else {
        return;
    };
    let scene = TownInfoScene::from_info(info);
    let spec = PopulationSpec::from_scene(&scene);
    let slots = citizen_slots(&scene, spec);
    let bounds = scene.bounds();
    let aabb = BoundingBox::new(
        Vector3::new(
            f64::from(bounds.min_x - 8),
            -64.0,
            f64::from(bounds.min_z - 8),
        ),
        Vector3::new(
            f64::from(bounds.max_x + 8),
            320.0,
            f64::from(bounds.max_z + 8),
        ),
    );
    let existing = world.get_entities_at_box(&aabb);
    let occupied = existing
        .iter()
        .filter(|e| crate::faction::is_kingdoms_tagged(e.get_entity()))
        .count();
    if occupied >= slots.len() {
        return;
    }
    let kingdom = scene.kingdom();
    let budget = director::recruit_budget(&kingdom);
    if budget == 0 {
        return;
    }
    let mut spawned = 0usize;
    for slot in slots {
        let home_chunk =
            pumpkin_util::math::vector2::Vector2::new(slot.home.x >> 4, slot.home.z >> 4);
        if !world.level.is_chunk_loaded(&home_chunk) {
            continue;
        }
        let already = existing.iter().any(|e| {
            let p = e.get_entity().pos.load();
            (p.x.floor() as i32 - slot.home.x).abs() <= 2
                && (p.z.floor() as i32 - slot.home.z).abs() <= 2
                && crate::faction::is_kingdoms_tagged(e.get_entity())
        });
        if already {
            continue;
        }
        let y = slot.home.height_or(
            world
                .sample_surface_height(slot.home.x, slot.home.z)
                .unwrap_or(world.sea_level)
                + 1,
        );
        let pos = Vector3::new(
            f64::from(slot.home.x) + 0.5,
            f64::from(y),
            f64::from(slot.home.z) + 0.5,
        );
        if director::spawn_citizen(world, town_id, &kingdom, slot.role, slot.is_golem, pos)
            .is_some()
        {
            spawned += 1;
            if spawned >= budget.min(RESPAWN_PER_TICK) {
                break;
            }
        }
    }
    if spawned > 0 {
        debug!(town = town_id, spawned, "respawned citizens from beds");
    }
}
