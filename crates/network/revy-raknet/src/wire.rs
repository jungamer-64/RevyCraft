use crate::RakNetError;
use crate::budget::{RakNetBudgets, ensure_budget};
use crate::sequence::{DatagramSequence, OrderSequence, ReliableSequence};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub(crate) const MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

pub(crate) enum OfflineRequest {
    Ping { time: i64 },
    OpenConnection1 { protocol: u8, mtu: u16 },
    OpenConnection2 { mtu: u16, client_guid: i64 },
}

pub(crate) fn decode_offline_request(bytes: &[u8]) -> Result<Option<OfflineRequest>, RakNetError> {
    let Some(id) = bytes.first().copied() else {
        return Ok(None);
    };
    let mut reader = Reader::new(bytes);
    let _ = reader.u8()?;
    match id {
        0x01 | 0x02 => {
            let time = reader.i64_be()?;
            reader.magic()?;
            let _client_guid = reader.i64_be()?;
            Ok(Some(OfflineRequest::Ping { time }))
        }
        0x05 => {
            reader.magic()?;
            let protocol = reader.u8()?;
            let mtu = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
            Ok(Some(OfflineRequest::OpenConnection1 { protocol, mtu }))
        }
        0x07 => {
            reader.magic()?;
            let _server_address = reader.address()?;
            let mtu = reader.u16_be()?;
            let client_guid = reader.i64_be()?;
            Ok(Some(OfflineRequest::OpenConnection2 { mtu, client_guid }))
        }
        _ => Ok(None),
    }
}

pub(crate) fn encode_unconnected_pong(
    time: i64,
    server_guid: i64,
    motd: &str,
) -> Result<Vec<u8>, RakNetError> {
    let motd = motd.as_bytes();
    let motd_len = u16::try_from(motd.len())
        .map_err(|_| RakNetError::Wire("raknet pong motd exceeded u16 length".to_string()))?;
    let mut output = Vec::with_capacity(35 + motd.len());
    output.push(0x1c);
    output.extend_from_slice(&time.to_be_bytes());
    output.extend_from_slice(&server_guid.to_be_bytes());
    output.extend_from_slice(&MAGIC);
    output.extend_from_slice(&motd_len.to_be_bytes());
    output.extend_from_slice(motd);
    Ok(output)
}

pub(crate) fn encode_open_connection_reply_1(server_guid: i64, mtu: u16) -> Vec<u8> {
    let mut output = Vec::with_capacity(28);
    output.push(0x06);
    output.extend_from_slice(&MAGIC);
    output.extend_from_slice(&server_guid.to_be_bytes());
    output.push(0);
    output.extend_from_slice(&mtu.to_be_bytes());
    output
}

pub(crate) fn encode_incompatible_protocol(protocol: u8, server_guid: i64) -> Vec<u8> {
    let mut output = Vec::with_capacity(26);
    output.push(0x19);
    output.push(protocol);
    output.extend_from_slice(&MAGIC);
    output.extend_from_slice(&server_guid.to_be_bytes());
    output
}

pub(crate) fn encode_open_connection_reply_2(
    server_guid: i64,
    client_addr: SocketAddr,
    mtu: u16,
) -> Vec<u8> {
    let mut output = Vec::with_capacity(48);
    output.push(0x08);
    output.extend_from_slice(&MAGIC);
    output.extend_from_slice(&server_guid.to_be_bytes());
    write_address(&mut output, client_addr);
    output.extend_from_slice(&mtu.to_be_bytes());
    output.push(0);
    output
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reliability {
    Unreliable,
    UnreliableSequenced,
    Reliable,
    ReliableOrdered,
    ReliableSequenced,
    UnreliableWithReceipt,
    ReliableWithReceipt,
    ReliableOrderedWithReceipt,
}

impl Reliability {
    fn from_tag(tag: u8) -> Result<Self, RakNetError> {
        match tag {
            0 => Ok(Self::Unreliable),
            1 => Ok(Self::UnreliableSequenced),
            2 => Ok(Self::Reliable),
            3 => Ok(Self::ReliableOrdered),
            4 => Ok(Self::ReliableSequenced),
            5 => Ok(Self::UnreliableWithReceipt),
            6 => Ok(Self::ReliableWithReceipt),
            7 => Ok(Self::ReliableOrderedWithReceipt),
            _ => Err(RakNetError::Wire(format!("invalid reliability tag {tag}"))),
        }
    }

    const fn tag(self) -> u8 {
        self as u8
    }

    pub(crate) const fn reliable(self) -> bool {
        matches!(
            self,
            Self::Reliable
                | Self::ReliableOrdered
                | Self::ReliableSequenced
                | Self::ReliableWithReceipt
                | Self::ReliableOrderedWithReceipt
        )
    }

    pub(crate) const fn sequenced(self) -> bool {
        matches!(self, Self::UnreliableSequenced | Self::ReliableSequenced)
    }

    pub(crate) const fn ordered(self) -> bool {
        matches!(
            self,
            Self::UnreliableSequenced
                | Self::ReliableOrdered
                | Self::ReliableSequenced
                | Self::ReliableOrderedWithReceipt
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SplitHeader {
    pub count: u32,
    pub id: u16,
    pub index: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct Frame {
    pub reliability: Reliability,
    pub reliable_index: Option<ReliableSequence>,
    pub sequence_index: Option<OrderSequence>,
    pub order_index: Option<OrderSequence>,
    pub order_channel: u8,
    pub split: Option<SplitHeader>,
    pub payload: Vec<u8>,
}

pub(crate) struct Datagram {
    pub sequence: DatagramSequence,
    pub frames: Vec<Frame>,
}

pub(crate) fn decode_datagram(
    bytes: &[u8],
    budgets: RakNetBudgets,
) -> Result<Datagram, RakNetError> {
    ensure_budget("datagram bytes", bytes.len(), budgets.max_datagram_bytes)?;
    let mut reader = Reader::new(bytes);
    let flags = reader.u8()?;
    if flags & 0x80 == 0 || flags & 0x60 != 0 {
        return Err(RakNetError::Wire(format!(
            "invalid datagram flags 0x{flags:02x}"
        )));
    }
    let sequence = DatagramSequence::new(reader.u24_le()?);
    let mut frames = Vec::new();
    while !reader.is_empty() {
        let frame_flags = reader.u8()?;
        let reliability = Reliability::from_tag(frame_flags >> 5)?;
        let bit_len = usize::from(reader.u16_be()?);
        let byte_len = bit_len.div_ceil(8);
        ensure_budget("frame payload bytes", byte_len, budgets.max_payload_bytes)?;
        let reliable_index = if reliability.reliable() {
            Some(ReliableSequence::new(reader.u24_le()?))
        } else {
            None
        };
        let sequence_index = if reliability.sequenced() {
            Some(OrderSequence::new(reader.u24_le()?))
        } else {
            None
        };
        let (order_index, order_channel) = if reliability.ordered() {
            (Some(OrderSequence::new(reader.u24_le()?)), reader.u8()?)
        } else {
            (None, 0)
        };
        let split = if frame_flags & 0x10 != 0 {
            Some(SplitHeader {
                count: reader.u32_be()?,
                id: reader.u16_be()?,
                index: reader.u32_be()?,
            })
        } else {
            None
        };
        let payload = reader.bytes(byte_len)?.to_vec();
        frames.push(Frame {
            reliability,
            reliable_index,
            sequence_index,
            order_index,
            order_channel,
            split,
            payload,
        });
    }
    Ok(Datagram { sequence, frames })
}

pub(crate) fn encode_datagram(sequence: DatagramSequence, frame: &Frame) -> Vec<u8> {
    let split_flag = if frame.split.is_some() { 0x10 } else { 0 };
    let mut output = Vec::with_capacity(frame.payload.len() + 32);
    output.push(0x84);
    write_u24_le(&mut output, sequence.value());
    output.push((frame.reliability.tag() << 5) | split_flag);
    let bit_len = u16::try_from(frame.payload.len().saturating_mul(8)).unwrap_or(u16::MAX);
    output.extend_from_slice(&bit_len.to_be_bytes());
    if let Some(index) = frame.reliable_index {
        write_u24_le(&mut output, index.value());
    }
    if let Some(index) = frame.sequence_index {
        write_u24_le(&mut output, index.value());
    }
    if let Some(index) = frame.order_index {
        write_u24_le(&mut output, index.value());
        output.push(frame.order_channel);
    }
    if let Some(split) = &frame.split {
        output.extend_from_slice(&split.count.to_be_bytes());
        output.extend_from_slice(&split.id.to_be_bytes());
        output.extend_from_slice(&split.index.to_be_bytes());
    }
    output.extend_from_slice(&frame.payload);
    output
}

pub(crate) enum Acknowledgement {
    Ack(Vec<(DatagramSequence, DatagramSequence)>),
    Nack(Vec<(DatagramSequence, DatagramSequence)>),
}

pub(crate) fn decode_acknowledgement(bytes: &[u8]) -> Result<Option<Acknowledgement>, RakNetError> {
    let Some(id) = bytes.first().copied() else {
        return Ok(None);
    };
    if !matches!(id, 0xa0 | 0xc0) {
        return Ok(None);
    }
    let mut reader = Reader::new(bytes);
    let _ = reader.u8()?;
    let count = usize::from(reader.u16_be()?);
    let mut ranges = Vec::with_capacity(count);
    for _ in 0..count {
        let single = reader.u8()? != 0;
        let start = DatagramSequence::new(reader.u24_le()?);
        let end = if single {
            start
        } else {
            DatagramSequence::new(reader.u24_le()?)
        };
        ranges.push((start, end));
    }
    Ok(Some(if id == 0xc0 {
        Acknowledgement::Ack(ranges)
    } else {
        Acknowledgement::Nack(ranges)
    }))
}

pub(crate) fn encode_ack(sequence: DatagramSequence) -> Vec<u8> {
    encode_ack_range(0xc0, sequence, sequence)
}

pub(crate) fn encode_nack(start: DatagramSequence, end: DatagramSequence) -> Vec<u8> {
    encode_ack_range(0xa0, start, end)
}

fn encode_ack_range(id: u8, start: DatagramSequence, end: DatagramSequence) -> Vec<u8> {
    let mut output = Vec::with_capacity(10);
    output.push(id);
    output.extend_from_slice(&1_u16.to_be_bytes());
    output.push(u8::from(start == end));
    write_u24_le(&mut output, start.value());
    if start != end {
        write_u24_le(&mut output, end.value());
    }
    output
}

pub(crate) fn encode_connection_accept(
    client_addr: SocketAddr,
    request_time: i64,
    timestamp: i64,
) -> Vec<u8> {
    let mut output = Vec::with_capacity(160);
    output.push(0x10);
    write_address(&mut output, client_addr);
    output.extend_from_slice(&0_u16.to_be_bytes());
    let unspecified = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
    for _ in 0..10 {
        write_address(&mut output, unspecified);
    }
    output.extend_from_slice(&request_time.to_be_bytes());
    output.extend_from_slice(&timestamp.to_be_bytes());
    output
}

pub(crate) fn encode_connected_pong(ping_time: i64, pong_time: i64) -> Vec<u8> {
    let mut output = Vec::with_capacity(17);
    output.push(0x03);
    output.extend_from_slice(&ping_time.to_be_bytes());
    output.extend_from_slice(&pong_time.to_be_bytes());
    output
}

pub(crate) fn decode_connection_request(payload: &[u8]) -> Result<Option<i64>, RakNetError> {
    if payload.first().copied() != Some(0x09) {
        return Ok(None);
    }
    let mut reader = Reader::new(payload);
    let _ = reader.u8()?;
    let _client_guid = reader.i64_be()?;
    let request_time = reader.i64_be()?;
    let _security = reader.u8()?;
    Ok(Some(request_time))
}

pub(crate) fn decode_connected_ping(payload: &[u8]) -> Result<Option<i64>, RakNetError> {
    if payload.first().copied() != Some(0x00) {
        return Ok(None);
    }
    let mut reader = Reader::new(payload);
    let _ = reader.u8()?;
    Ok(Some(reader.i64_be()?))
}

fn write_u24_le(output: &mut Vec<u8>, value: u32) {
    let bytes = value.to_le_bytes();
    output.extend_from_slice(&bytes[..3]);
}

fn write_address(output: &mut Vec<u8>, address: SocketAddr) {
    match address {
        SocketAddr::V4(address) => {
            output.push(4);
            output.extend(address.ip().octets().map(|octet| !octet));
            output.extend_from_slice(&address.port().to_be_bytes());
        }
        SocketAddr::V6(address) => {
            output.push(6);
            output.extend_from_slice(&23_u16.to_le_bytes());
            output.extend_from_slice(&address.port().to_be_bytes());
            output.extend_from_slice(&address.flowinfo().to_be_bytes());
            output.extend_from_slice(&address.ip().octets());
            output.extend_from_slice(&address.scope_id().to_be_bytes());
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn is_empty(&self) -> bool {
        self.cursor == self.bytes.len()
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], RakNetError> {
        let end = self
            .cursor
            .checked_add(len)
            .ok_or_else(|| RakNetError::Wire("raknet cursor overflowed".to_string()))?;
        let bytes = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| RakNetError::Wire("truncated raknet packet".to_string()))?;
        self.cursor = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, RakNetError> {
        Ok(self.bytes(1)?[0])
    }

    fn u16_be(&mut self) -> Result<u16, RakNetError> {
        Ok(u16::from_be_bytes(self.bytes(2)?.try_into().map_err(
            |_| RakNetError::Wire("truncated u16".to_string()),
        )?))
    }

    fn u24_le(&mut self) -> Result<u32, RakNetError> {
        let bytes = self.bytes(3)?;
        Ok(u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16))
    }

    fn u32_be(&mut self) -> Result<u32, RakNetError> {
        Ok(u32::from_be_bytes(self.bytes(4)?.try_into().map_err(
            |_| RakNetError::Wire("truncated u32".to_string()),
        )?))
    }

    fn i64_be(&mut self) -> Result<i64, RakNetError> {
        Ok(i64::from_be_bytes(self.bytes(8)?.try_into().map_err(
            |_| RakNetError::Wire("truncated i64".to_string()),
        )?))
    }

    fn magic(&mut self) -> Result<(), RakNetError> {
        if self.bytes(MAGIC.len())? == MAGIC {
            Ok(())
        } else {
            Err(RakNetError::Wire(
                "invalid raknet offline magic".to_string(),
            ))
        }
    }

    fn address(&mut self) -> Result<SocketAddr, RakNetError> {
        match self.u8()? {
            4 => {
                let octets = self.bytes(4)?;
                let ip = Ipv4Addr::new(!octets[0], !octets[1], !octets[2], !octets[3]);
                Ok(SocketAddr::new(IpAddr::V4(ip), self.u16_be()?))
            }
            6 => {
                let _family = self.bytes(2)?;
                let port = self.u16_be()?;
                let flowinfo = self.u32_be()?;
                let ip = Ipv6Addr::from(
                    <[u8; 16]>::try_from(self.bytes(16)?)
                        .map_err(|_| RakNetError::Wire("truncated ipv6 address".to_string()))?,
                );
                let scope_id = self.u32_be()?;
                Ok(SocketAddr::V6(std::net::SocketAddrV6::new(
                    ip, port, flowinfo, scope_id,
                )))
            }
            version => Err(RakNetError::Wire(format!(
                "unsupported raknet address version {version}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledgement_round_trips_single_sequence() {
        let sequence = DatagramSequence::new(0x12_3456);
        let encoded = encode_ack(sequence);
        let Some(Acknowledgement::Ack(ranges)) = decode_acknowledgement(&encoded).unwrap() else {
            panic!("ack should decode");
        };
        assert_eq!(ranges, vec![(sequence, sequence)]);
    }

    #[test]
    fn reliable_ordered_frame_round_trips() {
        let frame = Frame {
            reliability: Reliability::ReliableOrdered,
            reliable_index: Some(ReliableSequence::new(9)),
            sequence_index: None,
            order_index: Some(OrderSequence::new(4)),
            order_channel: 0,
            split: None,
            payload: vec![0xfe, 1, 2, 3],
        };
        let encoded = encode_datagram(DatagramSequence::new(7), &frame);
        let decoded = decode_datagram(&encoded, RakNetBudgets::default()).unwrap();
        assert_eq!(decoded.sequence, DatagramSequence::new(7));
        assert_eq!(decoded.frames[0].payload, frame.payload);
        assert_eq!(decoded.frames[0].order_index, frame.order_index);
    }
}
