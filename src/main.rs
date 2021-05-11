#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
extern crate simplelog;

use std::{
    self,
    collections::HashMap,
    iter::FromIterator,
    net::{
        TcpListener,
        ToSocketAddrs,
    },
    path::Path,
    sync::{
        mpsc::{
            self,
            Receiver,
            Sender,
        },
        Arc,
        RwLock,
    },
    thread,
};

use craftio_rs::{
    CraftConnection,
    CraftIo,
    CraftSyncReader,
    CraftSyncWriter,
    CraftTcpConnection,
};
use mcproto_rs::{
    protocol::{
        HasPacketId,
        Id,
        PacketDirection,
        PacketErr,
        RawPacket,
        State,
    },
    v1_16_3::{
        HandshakeNextState,
        HandshakeSpec,
        LoginStartSpec,
        Packet753 as PacketLatest,
        Packet753Kind as PacketLatestKind,
        PlayBlockChangeSpec,
        RawPacket753 as RawPacketLatest,
        StatusPongSpec,
        StatusResponseSpec,
    },
};
use simplelog::{
    ColorChoice,
    CombinedLogger,
    Config,
    LevelFilter,
    TermLogger,
    TerminalMode,
};

mod connection;
use crate::connection::{
    HasCraftConn,
    SplinterClientConnection,
    SplinterServerConnection,
};

mod config;
use crate::config::{
    ConfigLoadError,
    ConfigSaveError,
    SplinterProxyConfiguration,
};

mod mapping;
use crate::mapping::{
    process_raw_packet,
    MapAction,
    PacketMap,
};

enum EitherPacket {
    Normal(PacketLatest),
    // Raw(RawPacketLatest<'a>),
    Raw(Id, Vec<u8>),
}

fn get_config(config_path: &str) -> Arc<SplinterProxyConfiguration> {
    let config = match SplinterProxyConfiguration::load(Path::new(config_path)) {
        Ok(config) => {
            info!("Config loaded from {}", config_path);
            config
        }
        Err(ConfigLoadError::NoFile) => {
            warn!(
                "No config file found at {}. Creating a new one from defaults",
                config_path
            );
            let config = SplinterProxyConfiguration::default();
            match config.save(Path::new(config_path)) {
                Ok(()) => {}
                Err(ConfigSaveError::Create(e)) => {
                    error!("Failed to create file at {}: {}", config_path, e);
                }
                Err(ConfigSaveError::Write(e)) => {
                    error!("Failed to write to {}: {}", config_path, e);
                }
            }
            config
        }
        Err(ConfigLoadError::Io(e)) => {
            error!(
                "Failed to read config file at {}: {} Using default settings",
                config_path, e
            );
            SplinterProxyConfiguration::default()
        }
        Err(ConfigLoadError::De(e)) => {
            error!(
                "Failure to deserialize config file at {}: {}. Using default settings",
                config_path, e
            );
            SplinterProxyConfiguration::default()
        }
    };

    Arc::new(config)
}

fn main() {
    CombinedLogger::init(vec![TermLogger::new(
        LevelFilter::Trace,
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    )])
    .expect("Logger failed to initialize");
    info!("Starting Splinter proxy");

    let packet_map: Arc<RwLock<PacketMap>> = Arc::new(RwLock::new(HashMap::new()));
    {
        let mut map = packet_map.write().unwrap();

        // map.insert(
        //    PacketLatestKind::PlayBlockChange,
        //    Arc::new(|raw_packet: RawPacketLatest| {
        //        let packet = match raw_packet.deserialize() {
        //            Ok(packet) => packet,
        //            Err(e) => {
        //                error!("Failed to deserialize packet: {}", e);
        //                return MapAction::None;
        //            }
        //        };
        //        if let PacketLatest::PlayBlockChange(mut data) = packet {
        //            data.block_id = 5.into();
        //            MapAction::Client(PacketLatest::PlayBlockChange(data))
        //        } else {
        //            MapAction::Client(packet)
        //        }
        //    }),
        //);
    }
    let config = get_config("./config.ron");
    listen_for_clients(config, packet_map);
}

fn listen_for_clients(config: Arc<SplinterProxyConfiguration>, packet_map: Arc<RwLock<PacketMap>>) {
    let listener = match TcpListener::bind(&config.bind_address) {
        Err(e) => {
            return error!(
                "Failed to bind TCP listener to {}: {}",
                &config.bind_address, e
            )
        }
        Ok(listener) => listener,
    };
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                error!("Error when receiving incoming stream: {}", e);
                continue;
            }
        };

        let sock_addr = stream.peer_addr().unwrap();
        let craft_conn = match CraftConnection::from_std_with_state(
            stream,
            PacketDirection::ServerBound,
            State::Handshaking,
        ) {
            Ok(conn) => conn,
            Err(e) => {
                error!("Failed to wrap TCP stream {}: {}", sock_addr, e);
                continue;
            }
        };

        let conn = SplinterClientConnection {
            craft_conn: craft_conn,
            sock_addr: sock_addr,
            config: config.clone(),
        };

        info!("Got connection from {}", sock_addr);
        let packet_map = packet_map.clone();
        thread::spawn(move || await_handshake(conn, packet_map));
    }
}

fn await_handshake(mut conn: SplinterClientConnection, packet_map: Arc<RwLock<PacketMap>>) {
    match conn.craft_conn.read_raw_packet::<RawPacketLatest>() {
        Ok(Some(RawPacketLatest::Handshake(handshake_body))) => {
            match handshake_body.deserialize() {
                Ok(handshake) => {
                    debug!(
                        "received handshake from {}: ver {}, server {}:{}, next: {:?}",
                        conn.sock_addr,
                        handshake.version,
                        handshake.server_address,
                        handshake.server_port,
                        handshake.next_state
                    );
                    match handshake.next_state {
                        HandshakeNextState::Status => handle_status(conn),
                        HandshakeNextState::Login => handle_login(conn, packet_map),
                    }
                }
                Err(e) => {
                    error!(
                        "Error parsing handshake packet from {}: {}",
                        conn.sock_addr, e
                    )
                }
            }
        }
        Ok(Some(other)) => {
            error!("Unexpected packet from {}: {:?}", conn.sock_addr, other)
        }
        Ok(None) => info!("Connection with {} closed before handshake", conn.sock_addr),
        Err(e) => {
            error!("Error reading packet from {}: {}", conn.sock_addr, e)
        }
    }
}

fn handle_status(mut conn: SplinterClientConnection) {
    conn.craft_conn.set_state(State::Status);
    conn.write_packet(PacketLatest::StatusResponse(StatusResponseSpec {
        response: conn.config.server_status(None), // TODO: player count
    }));

    loop {
        match conn.craft_conn.read_raw_packet::<RawPacketLatest>() {
            Ok(Some(RawPacketLatest::StatusPing(body))) => match body.deserialize() {
                Ok(ping) => {
                    debug!("Got ping {} from {}", ping.payload, conn.sock_addr);
                    conn.write_packet(PacketLatest::StatusPong(StatusPongSpec {
                        payload: ping.payload,
                    }));
                }
                Err(e) => {
                    error!("Error parsing ping packet from {}: {}", conn.sock_addr, e)
                }
            },
            Ok(Some(other)) => {
                error!("Unexpected packet from {}: {:?}", conn.sock_addr, other)
            }
            Ok(None) => {
                info!("Connection with {} closed", conn.sock_addr);
                break;
            }
            Err(e) => error!("Error reading packet from {}: {}", conn.sock_addr, e),
        }
    }
}

fn handle_login(mut client: SplinterClientConnection, packet_map: Arc<RwLock<PacketMap>>) {
    client.craft_conn.set_state(State::Login);
    let logindata;
    loop {
        match client.craft_conn.read_raw_packet::<RawPacketLatest>() {
            Ok(Some(RawPacketLatest::LoginStart(body))) => match body.deserialize() {
                Ok(data) => {
                    logindata = data;
                    break;
                }
                Err(e) => {
                    return error!(
                        "Error parsing login start packet from {}: {}",
                        client.sock_addr, e
                    )
                }
            },
            Ok(Some(RawPacketLatest::Handshake(body))) => {
                warn!("Got a second handshake? {:?}", body.deserialize().unwrap());
            }
            Ok(Some(other)) => {
                return error!(
                    "Expected a login packet from {}, got {:?}",
                    client.sock_addr, other
                )
            }
            Ok(None) => {
                return info!(
                    "Connection to {} closed before login packet is received",
                    client.sock_addr
                )
            }
            Err(e) => return error!("Error reading packet from {}: {}", client.sock_addr, e),
        };
    }
    let name = logindata.name;
    info!(
        "\"{}\" is attempting to login from {}",
        name, client.sock_addr
    );
    debug!("Connecting \"{}\" to server", name);
    let server_addr = client
        .config
        .server_address
        .as_str()
        .to_socket_addrs() // yea
        .unwrap()
        .next()
        .unwrap();
    let craft_conn = match CraftTcpConnection::connect_server_std(server_addr) {
        Ok(conn) => conn,
        Err(e) => {
            return error!(
                "Failed to connect {} to server at {}: {}",
                name, server_addr, e
            )
        }
    };
    let mut server = SplinterServerConnection {
        craft_conn: craft_conn,
        sock_addr: server_addr,
    };

    server.write_packet(PacketLatest::Handshake(HandshakeSpec {
        version: client.config.protocol_version.into(),
        server_address: format!("{}", server_addr.ip()),
        server_port: server_addr.port(),
        next_state: HandshakeNextState::Login,
    }));

    server.craft_conn.set_state(State::Login);
    server.write_packet(PacketLatest::LoginStart(LoginStartSpec {
        name: name.clone(),
    }));
    // read login success
    match server.craft_conn.read_raw_packet::<RawPacketLatest>() {
        Ok(Some(RawPacketLatest::LoginSuccess(body))) => {
            match client
                .craft_conn
                .write_raw_packet(RawPacketLatest::LoginSuccess(body))
            {
                Ok(()) => {
                    trace!("Relaying login packet to client for {}", name)
                }
                Err(e) => return error!("Failed to relay login packet to client {}: {}", name, e),
            }
            client.craft_conn.set_state(State::Play);
            server.craft_conn.set_state(State::Play);
        }
        Ok(Some(packet)) => {
            return error!(
                "Expected a login success packet for {}, got {:?}",
                name, packet
            )
        }
        Ok(None) => {
            return info!(
                "Client connection closed before receiving login packet {}",
                name
            )
        }
        Err(e) => return error!("Failed to read packet from client for {}: {}", name, e),
    }

    let (mut server_reader, mut server_writer) = server.craft_conn.into_split(); // proxy's connection to the server
    let (mut client_reader, mut client_writer) = client.craft_conn.into_split(); // proxy's connection to the client
    let (server_writer_sender, server_writer_receiver) = mpsc::channel::<EitherPacket>();
    let (client_writer_sender, client_writer_receiver) = mpsc::channel::<EitherPacket>();
    let is_alive_arc = Arc::new(RwLock::new(true));
    // client reader
    {
        let packet_map = packet_map.clone();
        let is_alive_arc = is_alive_arc.clone();
        let name = name.clone();
        let server_writer_sender = server_writer_sender.clone();
        let client_writer_sender = client_writer_sender.clone();
        thread::spawn(move || {
            while *is_alive_arc.read().unwrap() {
                match client_reader.read_raw_packet::<RawPacketLatest>() {
                    Ok(Some(raw_packet)) => {
                        match process_raw_packet(&packet_map.read().unwrap(), raw_packet) {
                            MapAction::Relay(raw_packet) => {
                                match server_writer_sender.send(EitherPacket::Raw(
                                    raw_packet.id(),
                                    raw_packet.data().to_owned(),
                                )) {
                                    Ok(()) => {}
                                    Err(_e) => break,
                                }
                            }
                            MapAction::Server(packet) => {
                                match server_writer_sender.send(EitherPacket::Normal(packet)) {
                                    Ok(()) => {}
                                    Err(_e) => break,
                                }
                            }
                            MapAction::Client(packet) => {
                                match client_writer_sender.send(EitherPacket::Normal(packet)) {
                                    Ok(()) => {}
                                    Err(_e) => break,
                                }
                            }
                            MapAction::None => {}
                        }
                    }
                    Ok(None) => {
                        info!("Client connection closed for {}", name);
                        break;
                    }
                    Err(e) => {
                        // not sure if we should be doing something here
                        error!("Failed to read packet client to server for {}: {}", name, e);
                    }
                }
            }
            *is_alive_arc.write().unwrap() = false;
            trace!("client to server thread closed for {}", name);
        });
    }
    // server writer
    {
        let is_alive_arc = is_alive_arc.clone();
        let name = name.clone();
        thread::spawn(move || {
            let mut recv = server_writer_receiver.iter();
            while *is_alive_arc.read().unwrap() {
                match recv.next() {
                    Some(packet) => match match packet {
                        EitherPacket::Normal(packet) => server_writer.write_packet(packet),
                        EitherPacket::Raw(id, data) => {
                            match RawPacketLatest::create(id, data.as_slice()) {
                                Ok(packet) => server_writer.write_raw_packet(packet),
                                Err(_e) => continue,
                            }
                        }
                    } {
                        Ok(()) => {}
                        Err(e) => error!("Failed to relay packet to server for {}: {}", name, e),
                    },
                    None => break,
                }
            }
            *is_alive_arc.write().unwrap() = false;
        });
    }

    // server reader
    {
        let packet_map = packet_map.clone();
        let is_alive_arc = is_alive_arc.clone();
        let name = name.clone();
        let server_writer_sender = server_writer_sender.clone();
        let client_writer_sender = client_writer_sender.clone();
        thread::spawn(move || {
            while *is_alive_arc.read().unwrap() {
                match server_reader.read_raw_packet::<RawPacketLatest>() {
                    Ok(Some(raw_packet)) => {
                        match process_raw_packet(&packet_map.read().unwrap(), raw_packet) {
                            MapAction::Relay(raw_packet) => {
                                match client_writer_sender.send(EitherPacket::Raw(
                                    raw_packet.id(),
                                    raw_packet.data().to_owned(),
                                )) {
                                    Ok(()) => {}
                                    Err(_e) => break,
                                }
                            }
                            MapAction::Server(packet) => {
                                match server_writer_sender.send(EitherPacket::Normal(packet)) {
                                    Ok(()) => {}
                                    Err(_e) => break,
                                }
                            }
                            MapAction::Client(packet) => {
                                match client_writer_sender.send(EitherPacket::Normal(packet)) {
                                    Ok(()) => {}
                                    Err(_e) => break,
                                }
                            }
                            MapAction::None => {}
                        }
                    }
                    Ok(None) => {
                        info!("Server connection closed for {}", name);
                        break;
                    }
                    Err(e) => {
                        // not sure if we should be doing something here
                        error!("Failed to read packet server to client for {}: {}", name, e);
                    }
                };
            }
            *is_alive_arc.write().unwrap() = false;
            trace!("server to client thread closed for {}", name);
        });
    }
    // client writer
    let is_alive_arc = is_alive_arc.clone();
    let name = name.clone();
    thread::spawn(move || {
        let mut recv = client_writer_receiver.iter();
        while *is_alive_arc.read().unwrap() {
            match recv.next() {
                Some(packet) => match match packet {
                    EitherPacket::Normal(packet) => client_writer.write_packet(packet),
                    EitherPacket::Raw(id, data) => {
                        match RawPacketLatest::create(id, data.as_slice()) {
                            Ok(packet) => client_writer.write_raw_packet(packet),
                            Err(_e) => continue,
                        }
                    }
                } {
                    Ok(()) => {}
                    Err(e) => error!("Failed to relay packet to client for {}: {}", name, e),
                },
                None => break,
            }
        }
        *is_alive_arc.write().unwrap() = false;
    });
}
