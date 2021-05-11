pub type SplinterCraftConnection = CraftConnection<BufReader<TcpStream>, TcpStream>;

use std::{
    io::BufReader,
    net::{
        SocketAddr,
        TcpStream,
    },
    sync::Arc,
};

use craftio_rs::{
    CraftConnection,
    CraftSyncWriter,
};
use mcproto_rs::v1_16_3::{
    Packet753 as PacketLatest,
    RawPacket753 as RawPacketLatest,
};

use crate::config::SplinterProxyConfiguration;

pub struct SplinterClientConnection {
    pub craft_conn: SplinterCraftConnection,
    pub sock_addr: SocketAddr,
    pub config: Arc<SplinterProxyConfiguration>,
}

pub struct SplinterServerConnection {
    pub craft_conn: SplinterCraftConnection,
    pub sock_addr: SocketAddr,
}

pub trait HasCraftConn {
    fn craft_conn(&mut self) -> &mut SplinterCraftConnection;
    fn sock_addr(&self) -> SocketAddr;

    fn write_packet(&mut self, packet: PacketLatest) {
        match self.craft_conn().write_packet(packet) {
            Err(e) => return error!("Failed to write packet to {}: {}", self.sock_addr(), e),
            Ok(_) => {}
        }
    }

    fn write_raw_packet(&mut self, packet: RawPacketLatest) {
        match self.craft_conn().write_raw_packet(packet) {
            Err(e) => return error!("Failed to write packet to {}: {}", self.sock_addr(), e),
            Ok(_) => {}
        }
    }
}

impl HasCraftConn for SplinterClientConnection {
    fn craft_conn(&mut self) -> &mut SplinterCraftConnection {
        &mut self.craft_conn
    }

    fn sock_addr(&self) -> SocketAddr {
        self.sock_addr
    }
}

impl HasCraftConn for SplinterServerConnection {
    fn craft_conn(&mut self) -> &mut SplinterCraftConnection {
        &mut self.craft_conn
    }

    fn sock_addr(&self) -> SocketAddr {
        self.sock_addr
    }
}
