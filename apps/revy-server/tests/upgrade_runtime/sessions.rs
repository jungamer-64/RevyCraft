use crate::support::*;
use bytes::BytesMut;
use mc_proto_common::MinecraftWireCodec;
use mc_proto_test_support::{TestJavaPacket, TestJavaProtocol};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

pub(crate) struct JavaPlaySession {
    stream: TcpStream,
    buffer: BytesMut,
    codec: MinecraftWireCodec,
    protocol: TestJavaProtocol,
}

pub(crate) struct StatusSession {
    stream: TcpStream,
    buffer: BytesMut,
    codec: MinecraftWireCodec,
    protocol: TestJavaProtocol,
}

impl JavaPlaySession {
    pub(crate) fn connect(
        game_addr: SocketAddr,
        username: &str,
        context: &str,
    ) -> TestResult<Self> {
        let codec = MinecraftWireCodec;
        let protocol = TestJavaProtocol::Je5;
        let (stream, buffer) = connect_and_login_java_client(game_addr, &codec, protocol, username)
            .map_err(|error| format!("{context}: {error}"))?;
        Ok(Self {
            stream,
            buffer,
            codec,
            protocol,
        })
    }

    pub(crate) fn wait_for_bootstrap(&mut self, context: &str) -> TestResult<()> {
        let _ = self.read_packet(TestJavaPacket::HeldItemChange, 24, context)?;
        Ok(())
    }

    pub(crate) fn set_held_item(&mut self, slot: i16, context: &str) -> TestResult<()> {
        write_packet(&mut self.stream, &self.codec, &held_item_change(slot))
            .map_err(|error| format!("{context}: {error}").into())
    }

    pub(crate) fn read_held_item(&mut self, context: &str) -> TestResult<i16> {
        let packet = self.read_packet(TestJavaPacket::HeldItemChange, 24, context)?;
        Ok(held_item_from_packet(self.protocol, &packet)?.into())
    }

    pub(crate) fn assert_held_item_roundtrip(
        &mut self,
        slot: i16,
        write_context: &str,
        read_context: &str,
    ) -> TestResult<()> {
        self.set_held_item(slot, write_context)?;
        assert_eq!(self.read_held_item(read_context)?, slot);
        Ok(())
    }

    pub(crate) fn assert_no_packet(
        &mut self,
        packet: TestJavaPacket,
        timeout: Duration,
        context: &str,
    ) -> TestResult<()> {
        let packet_id = self
            .protocol
            .clientbound_packet_id(packet)
            .ok_or("packet should be supported")?;
        assert_no_packet_id(
            &mut self.stream,
            &self.codec,
            &mut self.buffer,
            packet_id,
            timeout,
        )
        .map_err(|error| format!("{context}: {error}").into())
    }

    pub(crate) fn connect_additional_player(
        game_addr: SocketAddr,
        username: &str,
        context: &str,
    ) -> TestResult<()> {
        let codec = MinecraftWireCodec;
        let protocol = TestJavaProtocol::Je5;
        let _ = connect_and_login_java_client(game_addr, &codec, protocol, username)
            .map_err(|error| format!("{context}: {error}"))?;
        Ok(())
    }

    fn read_packet(
        &mut self,
        packet: TestJavaPacket,
        max_frames: usize,
        context: &str,
    ) -> TestResult<Vec<u8>> {
        read_until_java_packet(
            &mut self.stream,
            &self.codec,
            &mut self.buffer,
            self.protocol,
            packet,
            Duration::from_secs(5),
            max_frames,
        )
        .map_err(|error| format!("{context}: {error}").into())
    }
}

impl StatusSession {
    pub(crate) fn connect(game_addr: SocketAddr, expected_motd: &str) -> TestResult<Self> {
        let codec = MinecraftWireCodec;
        let protocol = TestJavaProtocol::Je5;
        let mut stream = connect_tcp(game_addr)?;
        write_packet(
            &mut stream,
            &codec,
            &encode_handshake(protocol.protocol_version(), 1)?,
        )?;
        write_packet(&mut stream, &codec, &status_request())?;
        let mut session = Self {
            stream,
            buffer: BytesMut::new(),
            codec,
            protocol,
        };
        let response = session.read_packet(TestJavaPacket::StatusResponse)?;
        assert!(parse_status_response(&response)?.contains(expected_motd));
        Ok(session)
    }

    pub(crate) fn ping(&mut self, value: i64) -> TestResult<i64> {
        write_packet(&mut self.stream, &self.codec, &status_ping(value))?;
        let pong = self.read_packet(TestJavaPacket::StatusPong)?;
        parse_status_pong(&pong)
    }

    fn read_packet(&mut self, packet: TestJavaPacket) -> TestResult<Vec<u8>> {
        read_until_java_packet(
            &mut self.stream,
            &self.codec,
            &mut self.buffer,
            self.protocol,
            packet,
            Duration::from_secs(5),
            8,
        )
        .map_err(Into::into)
    }
}
