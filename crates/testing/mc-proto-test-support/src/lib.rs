use mc_proto_common::{PacketReader, PacketWriter, ProtocolError};

pub type TestItemStack = (i16, u8, i16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestJavaProtocol {
    Je5,
    Je47,
    Je340,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestJavaPacket {
    StatusResponse,
    StatusPong,
    LoginSuccess,
    JoinGame,
    SpawnPosition,
    PositionAndLook,
    NamedEntitySpawn,
    PlayerInfoAdd,
    EntityTeleport,
    BlockChange,
    OpenWindow,
    CloseWindow,
    SetSlot,
    WindowItems,
    WindowProperty,
    ConfirmTransaction,
    HeldItemChange,
    PlayerAbilities,
    ChunkData,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotNbtEncoding {
    LengthPrefixedBlob,
    RootTag,
}

#[derive(Debug, thiserror::Error)]
pub enum TestJavaProtocolError {
    #[error("{0}")]
    Message(&'static str),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

impl TestJavaProtocol {
    #[must_use]
    pub const fn protocol_version(self) -> i32 {
        match self {
            Self::Je5 => 5,
            Self::Je47 => 47,
            Self::Je340 => 340,
        }
    }

    #[must_use]
    pub const fn login_ready_packet_id(self) -> i32 {
        match self {
            Self::Je5 | Self::Je47 => 0x30,
            Self::Je340 => 0x14,
        }
    }

    #[must_use]
    pub const fn set_slot_packet_id(self) -> i32 {
        match self {
            Self::Je5 | Self::Je47 => 0x2f,
            Self::Je340 => 0x16,
        }
    }

    #[must_use]
    pub const fn window_items_packet_id(self) -> i32 {
        match self {
            Self::Je5 | Self::Je47 => 0x30,
            Self::Je340 => 0x14,
        }
    }

    #[must_use]
    pub const fn confirm_transaction_packet_id(self) -> i32 {
        match self {
            Self::Je5 | Self::Je47 => 0x32,
            Self::Je340 => 0x11,
        }
    }

    #[must_use]
    pub const fn clientbound_packet_id(self, packet: TestJavaPacket) -> Option<i32> {
        match (self, packet) {
            (_, TestJavaPacket::StatusResponse) => Some(0x00),
            (_, TestJavaPacket::StatusPong) => Some(0x01),
            (_, TestJavaPacket::LoginSuccess) => Some(0x02),
            (Self::Je5, TestJavaPacket::JoinGame) => Some(0x01),
            (Self::Je47, TestJavaPacket::JoinGame) => Some(0x01),
            (Self::Je340, TestJavaPacket::JoinGame) => Some(0x23),
            (Self::Je5, TestJavaPacket::SpawnPosition) => Some(0x05),
            (Self::Je47, TestJavaPacket::SpawnPosition) => Some(0x05),
            (Self::Je340, TestJavaPacket::SpawnPosition) => Some(0x46),
            (Self::Je5, TestJavaPacket::PositionAndLook) => Some(0x08),
            (Self::Je47, TestJavaPacket::PositionAndLook) => Some(0x08),
            (Self::Je340, TestJavaPacket::PositionAndLook) => Some(0x2f),
            (Self::Je5, TestJavaPacket::NamedEntitySpawn) => Some(0x0c),
            (Self::Je47, TestJavaPacket::NamedEntitySpawn) => Some(0x0c),
            (Self::Je340, TestJavaPacket::NamedEntitySpawn) => Some(0x05),
            (Self::Je5, TestJavaPacket::PlayerInfoAdd) => None,
            (Self::Je47, TestJavaPacket::PlayerInfoAdd) => Some(0x38),
            (Self::Je340, TestJavaPacket::PlayerInfoAdd) => Some(0x2d),
            (Self::Je5, TestJavaPacket::EntityTeleport) => Some(0x18),
            (Self::Je47, TestJavaPacket::EntityTeleport) => Some(0x18),
            (Self::Je340, TestJavaPacket::EntityTeleport) => Some(0x4c),
            (Self::Je5, TestJavaPacket::BlockChange) => Some(0x23),
            (Self::Je47, TestJavaPacket::BlockChange) => Some(0x23),
            (Self::Je340, TestJavaPacket::BlockChange) => Some(0x0b),
            (Self::Je5, TestJavaPacket::OpenWindow) => Some(0x2d),
            (Self::Je47, TestJavaPacket::OpenWindow) => Some(0x2d),
            (Self::Je340, TestJavaPacket::OpenWindow) => Some(0x13),
            (Self::Je5, TestJavaPacket::CloseWindow) => Some(0x2e),
            (Self::Je47, TestJavaPacket::CloseWindow) => Some(0x2e),
            (Self::Je340, TestJavaPacket::CloseWindow) => Some(0x12),
            (_, TestJavaPacket::SetSlot) => Some(self.set_slot_packet_id()),
            (_, TestJavaPacket::WindowItems) => Some(self.window_items_packet_id()),
            (Self::Je5, TestJavaPacket::WindowProperty) => Some(0x31),
            (Self::Je47, TestJavaPacket::WindowProperty) => Some(0x31),
            (Self::Je340, TestJavaPacket::WindowProperty) => Some(0x15),
            (_, TestJavaPacket::ConfirmTransaction) => Some(self.confirm_transaction_packet_id()),
            (Self::Je5, TestJavaPacket::HeldItemChange) => Some(0x09),
            (Self::Je47, TestJavaPacket::HeldItemChange) => Some(0x09),
            (Self::Je340, TestJavaPacket::HeldItemChange) => Some(0x3a),
            (Self::Je5, TestJavaPacket::PlayerAbilities) => Some(0x39),
            (Self::Je47, TestJavaPacket::PlayerAbilities) => Some(0x39),
            (Self::Je340, TestJavaPacket::PlayerAbilities) => Some(0x2c),
            (Self::Je5, TestJavaPacket::ChunkData) => Some(0x26),
            (Self::Je47, TestJavaPacket::ChunkData) => Some(0x21),
            (Self::Je340, TestJavaPacket::ChunkData) => Some(0x20),
        }
    }

    #[must_use]
    pub fn encode_creative_inventory_action(
        self,
        slot: i16,
        item_id: i16,
        count: u8,
        damage: i16,
    ) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(match self {
            Self::Je5 | Self::Je47 => 0x10,
            Self::Je340 => 0x1b,
        });
        writer.write_i16(slot);
        write_slot(
            &mut writer,
            Some((item_id, count, damage)),
            self.slot_nbt_encoding(),
        );
        writer.into_inner()
    }

    #[must_use]
    pub fn encode_click_window(
        self,
        slot: i16,
        button: i8,
        action_number: i16,
        clicked_item: Option<TestItemStack>,
    ) -> Vec<u8> {
        self.encode_click_window_in_window(0, slot, button, action_number, clicked_item)
    }

    #[must_use]
    pub fn encode_click_window_in_window(
        self,
        window_id: i8,
        slot: i16,
        button: i8,
        action_number: i16,
        clicked_item: Option<TestItemStack>,
    ) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(match self {
            Self::Je5 | Self::Je47 => 0x0e,
            Self::Je340 => 0x07,
        });
        writer.write_i8(window_id);
        writer.write_i16(slot);
        writer.write_i8(button);
        writer.write_i16(action_number);
        match self {
            Self::Je5 | Self::Je47 => writer.write_i8(0),
            Self::Je340 => writer.write_varint(0),
        }
        write_slot(&mut writer, clicked_item, self.slot_nbt_encoding());
        writer.into_inner()
    }

    #[must_use]
    pub fn encode_confirm_transaction_ack(
        self,
        window_id: u8,
        action_number: i16,
        accepted: bool,
    ) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(match self {
            Self::Je5 | Self::Je47 => 0x0f,
            Self::Je340 => 0x05,
        });
        writer.write_u8(window_id);
        writer.write_i16(action_number);
        writer.write_bool(accepted);
        writer.into_inner()
    }

    #[must_use]
    pub fn encode_close_window(self, window_id: u8) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(match self {
            Self::Je5 | Self::Je47 => 0x0d,
            Self::Je340 => 0x08,
        });
        writer.write_u8(window_id);
        writer.into_inner()
    }

    pub fn decode_set_slot(
        self,
        packet: &[u8],
    ) -> Result<(i8, i16, Option<TestItemStack>), TestJavaProtocolError> {
        let mut reader = PacketReader::new(packet);
        if reader.read_varint()? != self.set_slot_packet_id() {
            return Err(TestJavaProtocolError::Message("expected set slot packet"));
        }
        let window_id = reader.read_i8()?;
        let slot = reader.read_i16()?;
        let stack = read_slot(&mut reader, self.slot_nbt_encoding())?;
        Ok((window_id, slot, stack))
    }

    pub fn decode_confirm_transaction(
        self,
        packet: &[u8],
    ) -> Result<(u8, i16, bool), TestJavaProtocolError> {
        let mut reader = PacketReader::new(packet);
        if reader.read_varint()? != self.confirm_transaction_packet_id() {
            return Err(TestJavaProtocolError::Message(
                "expected confirm transaction packet",
            ));
        }
        let window_id = reader.read_u8()?;
        let action_number = reader.read_i16()?;
        let accepted = reader.read_bool()?;
        Ok((window_id, action_number, accepted))
    }

    pub fn decode_open_window(
        self,
        packet: &[u8],
    ) -> Result<(u8, String, String, u8, Option<bool>), TestJavaProtocolError> {
        let mut reader = PacketReader::new(packet);
        let expected_packet_id = self
            .clientbound_packet_id(TestJavaPacket::OpenWindow)
            .ok_or(TestJavaProtocolError::Message(
                "expected open window packet id",
            ))?;
        if reader.read_varint()? != expected_packet_id {
            return Err(TestJavaProtocolError::Message(
                "expected open window packet",
            ));
        }
        let window_id = reader.read_u8()?;
        let window_type = reader.read_string(64)?;
        let title = reader.read_string(256)?;
        let slot_count = reader.read_u8()?;
        let use_title = match self {
            Self::Je5 | Self::Je47 => Some(reader.read_bool()?),
            Self::Je340 => None,
        };
        Ok((window_id, window_type, title, slot_count, use_title))
    }

    pub fn decode_window_property(
        self,
        packet: &[u8],
    ) -> Result<(u8, i16, i16), TestJavaProtocolError> {
        let mut reader = PacketReader::new(packet);
        let expected_packet_id = self
            .clientbound_packet_id(TestJavaPacket::WindowProperty)
            .ok_or(TestJavaProtocolError::Message(
                "expected window property packet id",
            ))?;
        if reader.read_varint()? != expected_packet_id {
            return Err(TestJavaProtocolError::Message(
                "expected window property packet",
            ));
        }
        Ok((reader.read_u8()?, reader.read_i16()?, reader.read_i16()?))
    }

    pub fn decode_close_window(self, packet: &[u8]) -> Result<u8, TestJavaProtocolError> {
        let mut reader = PacketReader::new(packet);
        let expected_packet_id = self
            .clientbound_packet_id(TestJavaPacket::CloseWindow)
            .ok_or(TestJavaProtocolError::Message(
                "expected close window packet id",
            ))?;
        if reader.read_varint()? != expected_packet_id {
            return Err(TestJavaProtocolError::Message(
                "expected close window packet",
            ));
        }
        reader.read_u8().map_err(TestJavaProtocolError::from)
    }

    pub fn window_items_slot(
        self,
        packet: &[u8],
        wanted_slot: usize,
    ) -> Result<Option<TestItemStack>, TestJavaProtocolError> {
        let mut reader = PacketReader::new(packet);
        if reader.read_varint()? != self.window_items_packet_id() {
            return Err(TestJavaProtocolError::Message(
                "expected window items packet",
            ));
        }
        let _window_id = reader.read_u8()?;
        let count = usize::try_from(reader.read_i16()?)
            .map_err(|_| TestJavaProtocolError::Message("negative window item count"))?;
        if wanted_slot >= count {
            return Err(TestJavaProtocolError::Message("wanted slot out of bounds"));
        }
        for slot in 0..count {
            let item = read_slot(&mut reader, self.slot_nbt_encoding())?;
            if slot == wanted_slot {
                return Ok(item);
            }
        }
        Err(TestJavaProtocolError::Message("wanted slot missing"))
    }

    fn slot_nbt_encoding(self) -> SlotNbtEncoding {
        match self {
            Self::Je5 => SlotNbtEncoding::LengthPrefixedBlob,
            Self::Je47 | Self::Je340 => SlotNbtEncoding::RootTag,
        }
    }
}

fn write_slot(writer: &mut PacketWriter, stack: Option<TestItemStack>, slot_nbt: SlotNbtEncoding) {
    let Some((item_id, count, damage)) = stack else {
        writer.write_i16(-1);
        return;
    };
    writer.write_i16(item_id);
    writer.write_u8(count);
    writer.write_i16(damage);
    match slot_nbt {
        SlotNbtEncoding::LengthPrefixedBlob => writer.write_i16(-1),
        SlotNbtEncoding::RootTag => writer.write_u8(0),
    }
}

fn read_slot(
    reader: &mut PacketReader<'_>,
    slot_nbt: SlotNbtEncoding,
) -> Result<Option<TestItemStack>, TestJavaProtocolError> {
    let item_id = reader.read_i16()?;
    if item_id < 0 {
        return Ok(None);
    }
    let count = reader.read_u8()?;
    let damage = reader.read_i16()?;
    skip_slot_nbt(reader, slot_nbt)?;
    Ok(Some((item_id, count, damage)))
}

fn skip_slot_nbt(
    reader: &mut PacketReader<'_>,
    slot_nbt: SlotNbtEncoding,
) -> Result<(), TestJavaProtocolError> {
    match slot_nbt {
        SlotNbtEncoding::LengthPrefixedBlob => {
            let length = reader.read_i16()?;
            if length < 0 {
                return Ok(());
            }
            let length = usize::try_from(length)
                .map_err(|_| TestJavaProtocolError::Message("negative slot nbt length"))?;
            let _ = reader.read_bytes(length)?;
            Ok(())
        }
        SlotNbtEncoding::RootTag => {
            let tag_type = reader.read_u8()?;
            if tag_type == 0 {
                return Ok(());
            }
            skip_nbt_name(reader)?;
            skip_nbt_payload(reader, tag_type)
        }
    }
}

fn skip_nbt_name(reader: &mut PacketReader<'_>) -> Result<(), TestJavaProtocolError> {
    let length = usize::from(reader.read_u16()?);
    let _ = reader.read_bytes(length)?;
    Ok(())
}

fn skip_nbt_payload(
    reader: &mut PacketReader<'_>,
    tag_type: u8,
) -> Result<(), TestJavaProtocolError> {
    match tag_type {
        1 => {
            let _ = reader.read_u8()?;
        }
        2 => {
            let _ = reader.read_i16()?;
        }
        3 => {
            let _ = reader.read_i32()?;
        }
        4 => {
            let _ = reader.read_i64()?;
        }
        5 => {
            let _ = reader.read_f32()?;
        }
        6 => {
            let _ = reader.read_f64()?;
        }
        7 => skip_nbt_array(reader, 1)?,
        8 => skip_nbt_name(reader)?,
        9 => {
            let child_type = reader.read_u8()?;
            let len = read_nbt_length(reader)?;
            for _ in 0..len {
                skip_nbt_payload(reader, child_type)?;
            }
        }
        10 => loop {
            let child_type = reader.read_u8()?;
            if child_type == 0 {
                break;
            }
            skip_nbt_name(reader)?;
            skip_nbt_payload(reader, child_type)?;
        },
        11 => skip_nbt_array(reader, 4)?,
        12 => skip_nbt_array(reader, 8)?,
        _ => return Err(TestJavaProtocolError::Message("invalid slot nbt tag type")),
    }
    Ok(())
}

fn skip_nbt_array(
    reader: &mut PacketReader<'_>,
    element_width: usize,
) -> Result<(), TestJavaProtocolError> {
    let len = read_nbt_length(reader)?;
    let bytes = len
        .checked_mul(element_width)
        .ok_or(TestJavaProtocolError::Message("slot nbt array too large"))?;
    let _ = reader.read_bytes(bytes)?;
    Ok(())
}

fn read_nbt_length(reader: &mut PacketReader<'_>) -> Result<usize, TestJavaProtocolError> {
    usize::try_from(reader.read_i32()?)
        .map_err(|_| TestJavaProtocolError::Message("negative slot nbt length"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_constants_match_expected_versions() {
        assert_eq!(TestJavaProtocol::Je5.protocol_version(), 5);
        assert_eq!(TestJavaProtocol::Je47.protocol_version(), 47);
        assert_eq!(TestJavaProtocol::Je340.protocol_version(), 340);
        assert_eq!(TestJavaProtocol::Je5.login_ready_packet_id(), 0x30);
        assert_eq!(TestJavaProtocol::Je340.login_ready_packet_id(), 0x14);
        assert_eq!(
            TestJavaProtocol::Je47.clientbound_packet_id(TestJavaPacket::PlayerInfoAdd),
            Some(0x38)
        );
        assert_eq!(
            TestJavaProtocol::Je5.clientbound_packet_id(TestJavaPacket::PlayerInfoAdd),
            None
        );
    }

    #[test]
    fn legacy_set_slot_round_trips_with_length_prefixed_nbt() {
        let protocol = TestJavaProtocol::Je5;
        let mut writer = PacketWriter::default();
        writer.write_varint(protocol.set_slot_packet_id());
        writer.write_i8(0);
        writer.write_i16(36);
        write_slot(
            &mut writer,
            Some((17, 1, 0)),
            SlotNbtEncoding::LengthPrefixedBlob,
        );

        assert_eq!(
            protocol
                .decode_set_slot(&writer.into_inner())
                .expect("legacy set slot should decode"),
            (0, 36, Some((17, 1, 0)))
        );
    }

    #[test]
    fn modern_window_items_decode_uses_root_tag_slots() {
        let protocol = TestJavaProtocol::Je340;
        let mut writer = PacketWriter::default();
        writer.write_varint(protocol.window_items_packet_id());
        writer.write_u8(0);
        writer.write_i16(46);
        for _ in 0..45 {
            write_slot(&mut writer, None, SlotNbtEncoding::RootTag);
        }
        write_slot(&mut writer, Some((20, 64, 0)), SlotNbtEncoding::RootTag);

        assert_eq!(
            protocol
                .window_items_slot(&writer.into_inner(), 45)
                .expect("modern offhand slot should decode"),
            Some((20, 64, 0))
        );
    }

    #[test]
    fn click_window_packet_ids_follow_protocol_version() {
        let legacy = TestJavaProtocol::Je5.encode_click_window(36, 0, 1, None);
        let mut legacy_reader = PacketReader::new(&legacy);
        assert_eq!(
            legacy_reader
                .read_varint()
                .expect("legacy id should decode"),
            0x0e
        );
        assert_eq!(legacy_reader.read_i8().expect("window should decode"), 0);
        assert_eq!(legacy_reader.read_i16().expect("slot should decode"), 36);
        assert_eq!(legacy_reader.read_i8().expect("button should decode"), 0);
        assert_eq!(legacy_reader.read_i16().expect("action should decode"), 1);
        assert_eq!(legacy_reader.read_i8().expect("mode should decode"), 0);

        let modern = TestJavaProtocol::Je340.encode_click_window(36, 0, 1, None);
        let mut modern_reader = PacketReader::new(&modern);
        assert_eq!(
            modern_reader
                .read_varint()
                .expect("modern id should decode"),
            0x07
        );
        assert_eq!(modern_reader.read_i8().expect("window should decode"), 0);
        assert_eq!(modern_reader.read_i16().expect("slot should decode"), 36);
        assert_eq!(modern_reader.read_i8().expect("button should decode"), 0);
        assert_eq!(modern_reader.read_i16().expect("action should decode"), 1);
        assert_eq!(modern_reader.read_varint().expect("mode should decode"), 0);
    }
}
