use craftio_rs::CraftSyncWriter;
use mcproto_rs::{
    protocol::RawPacket,
    types::{
        BaseComponent,
        Chat,
        TextComponent,
    },
    v1_16_3::{
        ChatPosition,
        Packet753 as PacketLatest,
        Packet753Kind as PacketLatestKind,
        PlayClientChatMessageSpec,
        PlayServerChatMessageSpec,
        RawPacket753 as RawPacketLatest,
    },
};

use crate::{
    mapping::PacketMap,
    state::SplinterState,
};

/// Initializes chat handling
pub fn init(state: &mut SplinterState) {
    state.client_packet_map.insert(
        PacketLatestKind::PlayClientChatMessage,
        Box::new(|client, state, raw_packet| {
            match raw_packet.deserialize() {
                Ok(packet) => {
                    if let PacketLatest::PlayClientChatMessage(data) = packet {
                        info!("{}", data.message);
                        match data.message.get(..1) {
                            Some("/") => {
                                if let Err(e) = client
                                    .servers
                                    .read()
                                    .unwrap()
                                    .get(&0)
                                    .unwrap() // TODO: select the correct server
                                    .writer
                                    .lock()
                                    .unwrap()
                                    .write_packet(PacketLatest::PlayClientChatMessage(
                                        PlayClientChatMessageSpec {
                                            message: data.message,
                                        },
                                    ))
                                {
                                    error!(
                                        "Failed to send command message from {}: {}",
                                        client.name, e
                                    );
                                }
                            }
                            _ => {
                                let message = format!("{}: {}", client.name, data.message);
                                for (_id, target) in state.players.read().unwrap().iter() {
                                    if let Err(e) = target.writer.lock().unwrap().write_packet(
                                        PacketLatest::PlayServerChatMessage(
                                            PlayServerChatMessageSpec {
                                                message: Chat::Text(TextComponent {
                                                    text: message.clone(),
                                                    base: BaseComponent::default(),
                                                }),
                                                position: ChatPosition::ChatBox,
                                                sender: client.uuid,
                                            },
                                        ),
                                    ) {
                                        error!(
                                            "Failed to send chat message from {} to {}: {}",
                                            client.name, target.name, e
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                Err(e) => {
                    error!("failed to deserialize chat message from player: {}", e);
                }
            };
            false
        }),
    );
}
