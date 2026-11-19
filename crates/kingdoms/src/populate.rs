//! Spawn town citizens when a chunk is first generated.
//!
//! Home columns are a pure function of the plan, so each villager appears in
//! exactly one chunk. Reloaded chunks keep their entities; empty slots are
//! filled again by [`respawn_missing`].

use std::sync::Arc;

use pumpkin_core::block::entities::BlockEntity;
use pumpkin_core::block::entities::chest::ChestBlockEntity;
use pumpkin_core::plugin::api::events::world::ChunkPopulateEvent;
use pumpkin_core::plugin::{EventHandler, PluginFuture};
use pumpkin_core::server::Server;
use pumpkin_core::world::World;
use pumpkin_data::Block;
use pumpkin_data::dimension::Dimension;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_inventory::Inventory;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use tracing::debug;

use crate::director;
use crate::faction::{TAG_RAIDER, has_tag, is_kingdoms_tagged, is_pillager_type};
use crate::info::TownInfo;
use crate::roles::{PopulationSpec, citizen_slots};
use crate::scene::{TownInfoScene, TownScene};
use crate::town::towns_touching_chunk;

fn occupies_citizen_slot(entity: &pumpkin_core::entity::Entity) -> bool {
    is_kingdoms_tagged(entity)
        && !has_tag(entity, TAG_RAIDER)
        && !is_pillager_type(entity.entity_type)
}

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
        fill_armory_chests(world, &plan, chunk_x, chunk_z);
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

fn fill_armory_chests(world: &World, plan: &crate::plan::TownPlan, chunk_x: i32, chunk_z: i32) {
    let want = crate::blast::CHEST_TNT_COUNT;
    for rect in crate::blast::armory_rects(plan) {
        for (x, z) in crate::blast::chest_scan_columns(rect, chunk_x, chunk_z) {
            let ground = world.sample_surface_height(x, z).unwrap_or(world.sea_level);
            for y in ground.saturating_sub(1)..=ground.saturating_add(6) {
                let pos = BlockPos::new(x, y, z);
                if !world.is_loaded(&pos) {
                    continue;
                }
                let (block, _) = world.get_block_and_state(&pos);
                if block.id != Block::CHEST.id {
                    continue;
                }
                let inv = world.get_block_entity(&pos).map_or_else(
                    || {
                        let chest = Arc::new(ChestBlockEntity::new(pos));
                        world.add_block_entity(chest.clone());
                        BlockEntity::get_inventory(chest)
                    },
                    BlockEntity::get_inventory,
                );
                let Some(inv) = inv else {
                    continue;
                };
                if Inventory::count(inv.as_ref(), &Item::TNT) > 0 {
                    break;
                }
                for slot in 0..Inventory::size(inv.as_ref()) {
                    if Inventory::get_stack(inv.as_ref(), slot).is_empty() {
                        Inventory::set_stack(inv.as_ref(), slot, ItemStack::new(want, &Item::TNT));
                        break;
                    }
                }
                break;
            }
        }
    }
}

/// Max bodies to spawn on one director tick so a wipe does not hitch.
const RESPAWN_PER_TICK: usize = 2;

/// Fill empty citizen slots if the home chunk is loaded.
///
/// Deterministic homes mean a killed guard comes back at the same post.
/// Skipped while hostiles are inside the walls so capture holds are not
/// restocked mid-fight. A hinterland camp must not block civilian respawn.
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
    // Leftover raiders used to fill the bed count and block civilians, so the
    // bell never rang after a siege even once the alarm itself was clear.
    let occupied = existing
        .iter()
        .filter(|e| occupies_citizen_slot(e.get_entity()))
        .count();
    if occupied >= slots.len() {
        return;
    }
    let kingdom = scene.kingdom();
    let budget = director::recruit_budget_for_town(town_id);
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
            let base = e.get_entity();
            let p = base.pos.load();
            (p.x.floor() as i32 - slot.home.x).abs() <= 2
                && (p.z.floor() as i32 - slot.home.z).abs() <= 2
                && occupies_citizen_slot(base)
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
