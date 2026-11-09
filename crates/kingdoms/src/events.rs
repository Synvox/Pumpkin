//! Plugin event hooks: tick, populate, damage, death.

use std::sync::Arc;

use pumpkin_core::plugin::api::events::entity::entity_damage_by_entity::EntityDamageByEntityEvent;
use pumpkin_core::plugin::api::events::entity::entity_death::EntityDeathEvent;
use pumpkin_core::plugin::api::events::player::player_interact_event::{
    InteractAction, PlayerInteractEvent,
};
use pumpkin_core::plugin::api::events::player::player_join::PlayerJoinEvent;
use pumpkin_core::plugin::api::events::player::player_respawn::PlayerRespawnEvent;
use pumpkin_core::plugin::api::events::player::player_spawn_location::PlayerSpawnLocationEvent;
use pumpkin_core::plugin::api::events::server::ServerTickStartEvent;
use pumpkin_core::plugin::{EventHandler, EventPriority, Payload, PluginFuture, TypedEventHandler};
use pumpkin_core::server::Server;
use pumpkin_data::Block;

use crate::director;
use crate::faction::{is_allied, is_kingdoms_tagged};
use crate::populate::PopulateCitizens;

pub fn register(server: &Server) {
    push_handler(
        server,
        Arc::new(TypedEventHandler {
            handler: Arc::new(TickHandler),
            priority: EventPriority::Normal,
            blocking: true,
            source: Some("kingdoms".to_owned()),
            _phantom: std::marker::PhantomData,
        }),
    );
    push_handler(
        server,
        Arc::new(TypedEventHandler {
            handler: Arc::new(PopulateCitizens),
            priority: EventPriority::Normal,
            blocking: true,
            source: Some("kingdoms".to_owned()),
            _phantom: std::marker::PhantomData,
        }),
    );
    push_handler(
        server,
        Arc::new(TypedEventHandler {
            handler: Arc::new(FriendlyFireHandler),
            priority: EventPriority::High,
            blocking: true,
            source: Some("kingdoms".to_owned()),
            _phantom: std::marker::PhantomData,
        }),
    );
    push_handler(
        server,
        Arc::new(TypedEventHandler {
            handler: Arc::new(DeathHandler),
            priority: EventPriority::Normal,
            blocking: true,
            source: Some("kingdoms".to_owned()),
            _phantom: std::marker::PhantomData,
        }),
    );
    push_handler(
        server,
        Arc::new(TypedEventHandler {
            handler: Arc::new(JoinHandler),
            priority: EventPriority::Normal,
            blocking: true,
            source: Some("kingdoms".to_owned()),
            _phantom: std::marker::PhantomData,
        }),
    );
    push_handler(
        server,
        Arc::new(TypedEventHandler {
            handler: Arc::new(SpawnLocationHandler),
            priority: EventPriority::High,
            blocking: true,
            source: Some("kingdoms".to_owned()),
            _phantom: std::marker::PhantomData,
        }),
    );
    push_handler(
        server,
        Arc::new(TypedEventHandler {
            handler: Arc::new(RespawnHandler),
            priority: EventPriority::Normal,
            blocking: true,
            source: Some("kingdoms".to_owned()),
            _phantom: std::marker::PhantomData,
        }),
    );
    push_handler(
        server,
        Arc::new(TypedEventHandler {
            handler: Arc::new(InteractHandler),
            priority: EventPriority::Normal,
            blocking: true,
            source: Some("kingdoms".to_owned()),
            _phantom: std::marker::PhantomData,
        }),
    );
}

fn push_handler<E, H>(server: &Server, handler: Arc<TypedEventHandler<E, H>>)
where
    E: Payload + Send + Sync + 'static,
    H: EventHandler<E> + Send + Sync + 'static,
{
    server.plugin_manager.handlers.rcu(|handlers| {
        let mut next = (**handlers).clone();
        next.entry(E::get_name_static())
            .or_default()
            .push(handler.clone());
        Arc::new(next)
    });
}

struct TickHandler;

impl EventHandler<ServerTickStartEvent> for TickHandler {
    fn handle_blocking<'a>(
        &'a self,
        server: &'a Arc<Server>,
        _event: &'a mut ServerTickStartEvent,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            for world in server.worlds.load().iter() {
                director::tick_world(world);
            }
        })
    }
}

struct FriendlyFireHandler;

impl EventHandler<EntityDamageByEntityEvent> for FriendlyFireHandler {
    fn handle_blocking<'a>(
        &'a self,
        server: &'a Arc<Server>,
        event: &'a mut EntityDamageByEntityEvent,
    ) -> PluginFuture<'a, ()> {
        let victim_id = event.entity_id;
        let damager_id = event.damager_id;
        Box::pin(async move {
            for world in server.worlds.load().iter() {
                let Some(victim) = world.get_entity_by_id(victim_id) else {
                    continue;
                };
                let Some(damager) = world.get_entity_by_id(damager_id) else {
                    continue;
                };
                let dir = director::lock();
                let a = dir.faction_for(damager.as_ref());
                let b = dir.faction_for(victim.as_ref());
                drop(dir);
                if is_allied(&a, &b) {
                    event.cancelled = true;
                    event.damage = 0.0;
                } else {
                    if let Some(player) = damager.get_player() {
                        director::lock().note_hit(victim_id, player.gameprofile.id.to_string());
                    }
                    if (is_kingdoms_tagged(victim.get_entity())
                        || is_kingdoms_tagged(damager.get_entity()))
                        && damager_id % 17 == 0
                    {
                        tracing::info!(
                            "kingdoms combat attacker={damager_id} victim={victim_id} damage={}",
                            event.damage
                        );
                    }
                }
                return;
            }
        })
    }
}

struct JoinHandler;

impl EventHandler<PlayerJoinEvent> for JoinHandler {
    fn handle_blocking<'a>(
        &'a self,
        _server: &'a Arc<Server>,
        event: &'a mut PlayerJoinEvent,
    ) -> PluginFuture<'a, ()> {
        let player = Arc::clone(&event.player);
        Box::pin(async move {
            director::restore_player(&player);
        })
    }
}

struct SpawnLocationHandler;

impl EventHandler<PlayerSpawnLocationEvent> for SpawnLocationHandler {
    fn handle_blocking<'a>(
        &'a self,
        _server: &'a Arc<Server>,
        event: &'a mut PlayerSpawnLocationEvent,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            director::fix_spawn_location(&event.player, &mut event.spawn_pos);
        })
    }
}

struct RespawnHandler;

impl EventHandler<PlayerRespawnEvent> for RespawnHandler {
    fn handle_blocking<'a>(
        &'a self,
        _server: &'a Arc<Server>,
        event: &'a mut PlayerRespawnEvent,
    ) -> PluginFuture<'a, ()> {
        let player = Arc::clone(&event.player);
        Box::pin(async move {
            director::respawn_kit(&player);
        })
    }
}

struct InteractHandler;

impl EventHandler<PlayerInteractEvent> for InteractHandler {
    fn handle_blocking<'a>(
        &'a self,
        _server: &'a Arc<Server>,
        event: &'a mut PlayerInteractEvent,
    ) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            if event.action != InteractAction::RightClickBlock {
                return;
            }
            if event.block.id != Block::ANVIL.id && event.block.id != Block::DARK_OAK_LOG.id {
                return;
            }
            if let Ok(msg) = director::try_fire_trebuchet(&event.player) {
                event.player.send_system_message(
                    &pumpkin_util::text::TextComponent::text(msg)
                        .color_named(pumpkin_util::text::color::NamedColor::Gold),
                );
            }
        })
    }
}

struct DeathHandler;

impl EventHandler<EntityDeathEvent> for DeathHandler {
    fn handle_blocking<'a>(
        &'a self,
        _server: &'a Arc<Server>,
        event: &'a mut EntityDeathEvent,
    ) -> PluginFuture<'a, ()> {
        let id = event.entity_id;
        Box::pin(async move {
            director::note_entity_death(id);
        })
    }
}
