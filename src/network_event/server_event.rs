use std::fmt::Debug;

use bevy::{ecs::event::Event, prelude::*};
use bevy_renet::{
    renet::{RenetClient, RenetServer, SendType},
    transport::client_connected,
};
use bincode::{DefaultOptions, Options};
use serde::{
    de::{DeserializeOwned, DeserializeSeed},
    Serialize,
};

use super::{BuildEventDeserializer, BuildEventSerializer, EventChannel, MapEventEntities};
use crate::{
    client::{ClientSet, NetworkEntityMap},
    replication_core::NetworkChannels,
    server::{has_authority, ServerSet, SERVER_ID},
};

/// An extension trait for [`App`] for creating server events.
pub trait ServerEventAppExt {
    /// Registers event `T` that will be emitted on client after sending [`ToClients<T>`] on server.
    fn add_server_event<T: Event + Serialize + DeserializeOwned + Debug>(
        &mut self,
        policy: impl Into<SendType>,
    ) -> &mut Self;

    /// Same as [`Self::add_server_event`], but additionally maps server entities to client after receiving.
    fn add_mapped_server_event<T: Event + Serialize + DeserializeOwned + Debug + MapEventEntities>(
        &mut self,
        policy: impl Into<SendType>,
    ) -> &mut Self;

    /// Same as [`Self::add_server_event`], but the event will be serialized/deserialized using `S`/`D`
    /// with access to [`AppTypeRegistry`].
    ///
    /// Needed to send events that contain things like `Box<dyn Reflect>`.
    fn add_server_reflect_event<T, S, D>(&mut self, policy: impl Into<SendType>) -> &mut Self
    where
        T: Event + Debug,
        S: BuildEventSerializer<T> + 'static,
        D: BuildEventDeserializer + 'static,
        for<'a> S::EventSerializer<'a>: Serialize,
        for<'a, 'de> D::EventDeserializer<'a>: DeserializeSeed<'de, Value = T>;

    /// Same as [`Self::add_server_reflect_event`], but additionally maps client entities to client after receiving.
    fn add_mapped_server_reflect_event<T, S, D>(
        &mut self,
        policy: impl Into<SendType>,
    ) -> &mut Self
    where
        T: Event + Debug + MapEventEntities,
        S: BuildEventSerializer<T> + 'static,
        D: BuildEventDeserializer + 'static,
        for<'a> S::EventSerializer<'a>: Serialize,
        for<'a, 'de> D::EventDeserializer<'a>: DeserializeSeed<'de, Value = T>;

    /// Same as [`Self::add_server_event`], but uses specified sending and receiving systems.
    fn add_server_event_with<T: Event + Debug, Marker1, Marker2>(
        &mut self,
        policy: impl Into<SendType>,
        sending_system: impl IntoSystemConfigs<Marker1>,
        receiving_system: impl IntoSystemConfigs<Marker2>,
    ) -> &mut Self;
}

impl ServerEventAppExt for App {
    fn add_server_event<T: Event + Serialize + DeserializeOwned + Debug>(
        &mut self,
        policy: impl Into<SendType>,
    ) -> &mut Self {
        self.add_server_event_with::<T, _, _>(policy, sending_system::<T>, receiving_system::<T>)
    }

    fn add_mapped_server_event<
        T: Event + Serialize + DeserializeOwned + Debug + MapEventEntities,
    >(
        &mut self,
        policy: impl Into<SendType>,
    ) -> &mut Self {
        self.add_server_event_with::<T, _, _>(
            policy,
            sending_system::<T>,
            receiving_and_mapping_system::<T>,
        )
    }

    fn add_server_reflect_event<T, S, D>(&mut self, policy: impl Into<SendType>) -> &mut Self
    where
        T: Event + Debug,
        S: BuildEventSerializer<T> + 'static,
        D: BuildEventDeserializer + 'static,
        for<'a> S::EventSerializer<'a>: Serialize,
        for<'a, 'de> D::EventDeserializer<'a>: DeserializeSeed<'de, Value = T>,
    {
        self.add_server_event_with::<T, _, _>(
            policy,
            sending_reflect_system::<T, S>,
            receiving_reflect_system::<T, D>,
        )
    }

    fn add_mapped_server_reflect_event<T, S, D>(&mut self, policy: impl Into<SendType>) -> &mut Self
    where
        T: Event + Debug + MapEventEntities,
        S: BuildEventSerializer<T> + 'static,
        D: BuildEventDeserializer + 'static,
        for<'a> S::EventSerializer<'a>: Serialize,
        for<'a, 'de> D::EventDeserializer<'a>: DeserializeSeed<'de, Value = T>,
    {
        self.add_server_event_with::<T, _, _>(
            policy,
            sending_reflect_system::<T, S>,
            receiving_and_mapping_reflect_system::<T, D>,
        )
    }

    fn add_server_event_with<T: Event + Debug, Marker1, Marker2>(
        &mut self,
        policy: impl Into<SendType>,
        sending_system: impl IntoSystemConfigs<Marker1>,
        receiving_system: impl IntoSystemConfigs<Marker2>,
    ) -> &mut Self {
        let channel_id = self
            .world
            .resource_mut::<NetworkChannels>()
            .create_server_channel(policy.into());

        self.add_event::<T>()
            .init_resource::<Events<ToClients<T>>>()
            .insert_resource(EventChannel::<T>::new(channel_id))
            .add_systems(
                PreUpdate,
                receiving_system
                    .in_set(ClientSet::Receive)
                    .run_if(client_connected()),
            )
            .add_systems(
                PostUpdate,
                (
                    sending_system.run_if(resource_exists::<RenetServer>()),
                    local_resending_system::<T>.run_if(has_authority()),
                )
                    .chain()
                    .in_set(ServerSet::Send),
            );

        self
    }
}

fn receiving_system<T: Event + DeserializeOwned + Debug>(
    mut server_events: EventWriter<T>,
    mut client: ResMut<RenetClient>,
    channel: Res<EventChannel<T>>,
) {
    while let Some(message) = client.receive_message(channel.id) {
        let event = bincode::deserialize(&message).expect("server should send valid events");
        debug!("received event {event:?} from server");
        server_events.send(event);
    }
}

fn receiving_and_mapping_system<T: Event + MapEventEntities + DeserializeOwned + Debug>(
    mut server_events: EventWriter<T>,
    mut client: ResMut<RenetClient>,
    entity_map: Res<NetworkEntityMap>,
    channel: Res<EventChannel<T>>,
) {
    while let Some(message) = client.receive_message(channel.id) {
        let mut event: T =
            bincode::deserialize(&message).expect("server should send valid mapped events");
        debug!("received mapped event {event:?} from server");
        event
            .map_entities(entity_map.to_client())
            .unwrap_or_else(|e| panic!("server event {event:?} should be mappable: {e}"));
        server_events.send(event);
    }
}

fn receiving_reflect_system<T, D>(
    mut server_events: EventWriter<T>,
    mut client: ResMut<RenetClient>,
    channel: Res<EventChannel<T>>,
    registry: Res<AppTypeRegistry>,
) where
    T: Event + Debug,
    D: BuildEventDeserializer,
    for<'a, 'de> D::EventDeserializer<'a>: DeserializeSeed<'de, Value = T>,
{
    let registry = registry.read();
    while let Some(message) = client.receive_message(channel.id) {
        // Set options to match `bincode::serialize`.
        // https://docs.rs/bincode/latest/bincode/config/index.html#options-struct-vs-bincode-functions
        let options = DefaultOptions::new()
            .with_fixint_encoding()
            .allow_trailing_bytes();
        let mut deserializer = bincode::Deserializer::from_slice(&message, options);
        let event = D::new(&registry)
            .deserialize(&mut deserializer)
            .expect("server should send valid reflect events");
        debug!("received reflect event {event:?} from server");
        server_events.send(event);
    }
}

fn receiving_and_mapping_reflect_system<T, D>(
    mut server_events: EventWriter<T>,
    mut client: ResMut<RenetClient>,
    entity_map: Res<NetworkEntityMap>,
    channel: Res<EventChannel<T>>,
    registry: Res<AppTypeRegistry>,
) where
    T: Event + MapEventEntities + Debug,
    D: BuildEventDeserializer,
    for<'a, 'de> D::EventDeserializer<'a>: DeserializeSeed<'de, Value = T>,
{
    let registry = registry.read();
    while let Some(message) = client.receive_message(channel.id) {
        // Set options to match `bincode::serialize`.
        // https://docs.rs/bincode/latest/bincode/config/index.html#options-struct-vs-bincode-functions
        let options = DefaultOptions::new()
            .with_fixint_encoding()
            .allow_trailing_bytes();
        let mut deserializer = bincode::Deserializer::from_slice(&message, options);
        let mut event = D::new(&registry)
            .deserialize(&mut deserializer)
            .expect("server should send valid mapped reflect events");
        debug!("received mapped reflect event {event:?} from server");
        event
            .map_entities(entity_map.to_client())
            .unwrap_or_else(|e| panic!("server reflect event {event:?} should be mappable: {e}"));
        server_events.send(event);
    }
}

fn sending_system<T: Event + Serialize + Debug>(
    mut server: ResMut<RenetServer>,
    mut server_events: EventReader<ToClients<T>>,
    channel: Res<EventChannel<T>>,
) {
    for ToClients { event, mode } in &mut server_events {
        let message = bincode::serialize(&event).expect("server event should be serializable");

        match *mode {
            SendMode::Broadcast => {
                server.broadcast_message(channel.id, message);
                debug!("broadcasted server event {event:?}");
            }
            SendMode::BroadcastExcept(client_id) => {
                if client_id == SERVER_ID {
                    server.broadcast_message(channel.id, message);
                } else {
                    server.broadcast_message_except(client_id, channel.id, message);
                }
                debug!("broadcasted server event {event:?} except client {client_id}");
            }
            SendMode::Direct(client_id) => {
                if client_id != SERVER_ID {
                    server.send_message(client_id, channel.id, message);
                    debug!("sent direct server event {event:?} to client {client_id}");
                }
            }
        }
    }
}

fn sending_reflect_system<T, S>(
    mut server: ResMut<RenetServer>,
    mut server_events: EventReader<ToClients<T>>,
    channel: Res<EventChannel<T>>,
    registry: Res<AppTypeRegistry>,
) where
    T: Event + Debug,
    S: BuildEventSerializer<T>,
    for<'a> S::EventSerializer<'a>: Serialize,
{
    let registry = registry.read();
    for ToClients { event, mode } in &mut server_events {
        let serializer = S::new(event, &registry);
        let message = bincode::serialize(&serializer).expect("server event should be serializable");

        match *mode {
            SendMode::Broadcast => {
                server.broadcast_message(channel.id, message);
                debug!("broadcasted server reflect event {event:?}");
            }
            SendMode::BroadcastExcept(client_id) => {
                if client_id == SERVER_ID {
                    server.broadcast_message(channel.id, message);
                } else {
                    server.broadcast_message_except(client_id, channel.id, message);
                }
                debug!("broadcasted server reflect event {event:?} except client {client_id}");
            }
            SendMode::Direct(client_id) => {
                if client_id != SERVER_ID {
                    server.send_message(client_id, channel.id, message);
                    debug!("sent direct server reflect event {event:?} to client {client_id}");
                }
            }
        }
    }
}

/// Transforms [`ToClients<T>`] events into `T` events to "emulate"
/// message sending for offline mode or when server is also a player
fn local_resending_system<T: Event + Debug>(
    mut server_events: ResMut<Events<ToClients<T>>>,
    mut local_events: EventWriter<T>,
) {
    for ToClients { event, mode } in server_events.drain() {
        match mode {
            SendMode::Broadcast => {
                debug!("converted broadcasted server event {event:?} into a local");
                local_events.send(event);
            }
            SendMode::BroadcastExcept(client_id) => {
                if client_id != SERVER_ID {
                    debug!("converted broadcasted server event {event:?} except client {client_id} into a local");
                    local_events.send(event);
                }
            }
            SendMode::Direct(client_id) => {
                if client_id == SERVER_ID {
                    debug!("converted direct server event {event:?} into a local");
                    local_events.send(event);
                }
            }
        }
    }
}

/// An event that will be send to client(s).
#[derive(Clone, Copy, Debug, Event)]
pub struct ToClients<T> {
    pub mode: SendMode,
    pub event: T,
}

/// Type of server message sending.
#[derive(Clone, Copy, Debug)]
pub enum SendMode {
    Broadcast,
    BroadcastExcept(u64),
    Direct(u64),
}
