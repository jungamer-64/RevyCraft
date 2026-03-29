use num_traits::ToPrimitive;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};

const CHUNK_WIDTH: i32 = 16;
const SECTION_HEIGHT: i32 = 16;
const PLAYER_INVENTORY_SLOT_COUNT: usize = 45;
const AUXILIARY_SLOT_COUNT: u8 = 9;
const MAIN_INVENTORY_SLOT_COUNT: u8 = 27;
const HOTBAR_START_SLOT: u8 = 36;
const HOTBAR_SLOT_COUNT: u8 = 9;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlockKey(String);

impl BlockKey {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockState {
    pub key: BlockKey,
    pub properties: BTreeMap<String, String>,
}

impl BlockState {
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: BlockKey::new(key),
            properties: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockFace {
    Bottom,
    Top,
    North,
    South,
    West,
    East,
}

impl BlockFace {
    #[must_use]
    pub const fn from_protocol_byte(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Bottom),
            1 => Some(Self::Top),
            2 => Some(Self::North),
            3 => Some(Self::South),
            4 => Some(Self::West),
            5 => Some(Self::East),
            _ => None,
        }
    }

    #[must_use]
    pub const fn offset(self) -> (i32, i32, i32) {
        match self {
            Self::Bottom => (0, -1, 0),
            Self::Top => (0, 1, 0),
            Self::North => (0, 0, -1),
            Self::South => (0, 0, 1),
            Self::West => (-1, 0, 0),
            Self::East => (1, 0, 0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub const fn chunk_pos(self) -> ChunkPos {
        ChunkPos::new(
            self.x.div_euclid(CHUNK_WIDTH),
            self.z.div_euclid(CHUNK_WIDTH),
        )
    }

    #[must_use]
    pub const fn offset(self, face: BlockFace) -> Self {
        let (dx, dy, dz) = face.offset();
        Self::new(self.x + dx, self.y + dy, self.z + dz)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub fn chunk_pos(self) -> ChunkPos {
        ChunkPos::from_world_position(self.x, self.z)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DimensionId {
    Overworld,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldMeta {
    pub level_name: String,
    pub seed: u64,
    pub spawn: BlockPos,
    pub dimension: DimensionId,
    pub age: i64,
    pub time: i64,
    pub level_type: String,
    pub game_mode: u8,
    pub difficulty: u8,
    pub max_players: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl Serialize for ChunkPos {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{},{}", self.x, self.z))
    }
}

impl<'de> Deserialize<'de> for ChunkPos {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let Some((x, z)) = value.split_once(',') else {
            return Err(serde::de::Error::custom("invalid chunk coordinate"));
        };
        Ok(Self {
            x: x.parse().map_err(serde::de::Error::custom)?,
            z: z.parse().map_err(serde::de::Error::custom)?,
        })
    }
}

impl ChunkPos {
    #[must_use]
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    #[must_use]
    pub fn from_world_position(x: f64, z: f64) -> Self {
        let block_x = floor_to_i32(x);
        let block_z = floor_to_i32(z);
        Self {
            x: block_x.div_euclid(CHUNK_WIDTH),
            z: block_z.div_euclid(CHUNK_WIDTH),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SectionPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionPos {
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SectionBlockIndex(u16);

impl SectionBlockIndex {
    #[must_use]
    pub fn new(x: u8, y: u8, z: u8) -> Self {
        debug_assert!(
            x < 16,
            "chunk-local x coordinate should stay within a section"
        );
        debug_assert!(
            y < 16,
            "chunk-local y coordinate should stay within a section"
        );
        debug_assert!(
            z < 16,
            "chunk-local z coordinate should stay within a section"
        );
        Self(u16::from(y) << 8 | u16::from(z) << 4 | u16::from(x))
    }

    #[must_use]
    pub const fn from_raw(raw: u16) -> Self {
        Self(raw & 0x0fff)
    }

    #[must_use]
    pub const fn into_raw(self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn expand(self) -> (u8, u8, u8) {
        let x = (self.0 & 0x0F) as u8;
        let z = ((self.0 >> 4) & 0x0F) as u8;
        let y = ((self.0 >> 8) & 0x0F) as u8;
        (x, y, z)
    }
}

impl From<SectionBlockIndex> for u16 {
    fn from(index: SectionBlockIndex) -> Self {
        index.into_raw()
    }
}

impl From<SectionBlockIndex> for usize {
    fn from(index: SectionBlockIndex) -> Self {
        Self::from(index.into_raw())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkSection {
    pub y: i32,
    blocks: BTreeMap<SectionBlockIndex, BlockState>,
}

impl ChunkSection {
    #[must_use]
    pub const fn new(y: i32) -> Self {
        Self {
            y,
            blocks: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn get_block(&self, x: u8, y: u8, z: u8) -> Option<&BlockState> {
        self.blocks.get(&flatten_block_index(x, y, z))
    }

    pub fn set_block(&mut self, x: u8, y: u8, z: u8, state: Option<BlockState>) {
        let index = flatten_block_index(x, y, z);
        if let Some(state) = state {
            self.blocks.insert(index, state);
        } else {
            self.blocks.remove(&index);
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn iter_blocks(&self) -> impl Iterator<Item = (SectionBlockIndex, &BlockState)> {
        self.blocks.iter().map(|(index, state)| (*index, state))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkColumn {
    pub pos: ChunkPos,
    pub sections: BTreeMap<i32, ChunkSection>,
    pub biomes: Vec<u8>,
}

impl ChunkColumn {
    #[must_use]
    pub fn new(pos: ChunkPos) -> Self {
        Self {
            pos,
            sections: BTreeMap::new(),
            biomes: vec![1; 256],
        }
    }

    #[must_use]
    pub fn get_block(&self, x: u8, y: i32, z: u8) -> Option<BlockState> {
        let section_y = y.div_euclid(SECTION_HEIGHT);
        let local_y = section_local_y(y);
        self.sections
            .get(&section_y)
            .and_then(|section| section.get_block(x, local_y, z))
            .cloned()
    }

    pub fn set_block(&mut self, x: u8, y: i32, z: u8, state: Option<BlockState>) {
        let section_y = y.div_euclid(SECTION_HEIGHT);
        let local_y = section_local_y(y);
        if let Some(state) = state {
            let section = self
                .sections
                .entry(section_y)
                .or_insert_with(|| ChunkSection::new(section_y));
            section.set_block(x, local_y, z, Some(state));
            if section.is_empty() {
                self.sections.remove(&section_y);
            }
            return;
        }
        let Some(section) = self.sections.get_mut(&section_y) else {
            return;
        };
        section.set_block(x, local_y, z, None);
        if section.is_empty() {
            self.sections.remove(&section_y);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkDelta {
    pub added: Vec<ChunkPos>,
    pub removed: Vec<ChunkPos>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ItemKey(String);

impl ItemKey {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    pub key: ItemKey,
    pub count: u8,
    pub damage: u16,
}

impl ItemStack {
    #[must_use]
    pub fn new(key: impl Into<String>, count: u8, damage: u16) -> Self {
        Self {
            key: ItemKey::new(key),
            count,
            damage,
        }
    }

    #[must_use]
    pub fn unsupported(count: u8, damage: u16) -> Self {
        Self::new("minecraft:unsupported", count, damage)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerInventory {
    pub slots: Vec<Option<ItemStack>>,
    #[serde(default)]
    pub offhand: Option<ItemStack>,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self::new_empty()
    }
}

impl PlayerInventory {
    #[must_use]
    pub fn new_empty() -> Self {
        Self {
            slots: vec![None; PLAYER_INVENTORY_SLOT_COUNT],
            offhand: None,
        }
    }

    #[must_use]
    pub fn get(&self, slot: u8) -> Option<&ItemStack> {
        self.slots
            .get(usize::from(slot))
            .and_then(std::option::Option::as_ref)
    }

    pub fn set(&mut self, slot: u8, stack: Option<ItemStack>) -> bool {
        if usize::from(slot) >= PLAYER_INVENTORY_SLOT_COUNT {
            return false;
        }
        self.slots[usize::from(slot)] = stack;
        true
    }

    #[must_use]
    pub fn selected_hotbar_stack(&self, selected_hotbar_slot: u8) -> Option<&ItemStack> {
        if selected_hotbar_slot >= HOTBAR_SLOT_COUNT {
            return None;
        }
        self.get(HOTBAR_START_SLOT + selected_hotbar_slot)
    }

    #[must_use]
    pub fn get_slot(&self, slot: InventorySlot) -> Option<&ItemStack> {
        match slot {
            InventorySlot::Offhand => self.offhand.as_ref(),
            _ => slot
                .legacy_window_index()
                .and_then(|legacy_slot| self.get(legacy_slot)),
        }
    }

    pub fn set_slot(&mut self, slot: InventorySlot, stack: Option<ItemStack>) -> bool {
        match slot {
            InventorySlot::Offhand => {
                self.offhand = stack;
                true
            }
            _ => slot
                .legacy_window_index()
                .is_some_and(|legacy_slot| self.set(legacy_slot, stack)),
        }
    }

    pub fn get_slot_mut(&mut self, slot: InventorySlot) -> Option<&mut Option<ItemStack>> {
        match slot {
            InventorySlot::Offhand => Some(&mut self.offhand),
            _ => slot
                .legacy_window_index()
                .and_then(|legacy_slot| self.slots.get_mut(usize::from(legacy_slot))),
        }
    }

    #[must_use]
    pub fn selected_stack(
        &self,
        selected_hotbar_slot: u8,
        hand: InteractionHand,
    ) -> Option<&ItemStack> {
        match hand {
            InteractionHand::Main => self.selected_hotbar_stack(selected_hotbar_slot),
            InteractionHand::Offhand => self.offhand.as_ref(),
        }
    }

    #[must_use]
    pub fn crafting_result(&self) -> Option<&ItemStack> {
        self.get_slot(InventorySlot::crafting_result())
    }

    pub fn set_crafting_result(&mut self, stack: Option<ItemStack>) -> bool {
        self.set_slot(InventorySlot::crafting_result(), stack)
    }

    #[must_use]
    pub fn crafting_input(&self, index: u8) -> Option<&ItemStack> {
        InventorySlot::crafting_input(index).and_then(|slot| self.get_slot(slot))
    }

    pub fn set_crafting_input(&mut self, index: u8, stack: Option<ItemStack>) -> bool {
        InventorySlot::crafting_input(index).is_some_and(|slot| self.set_slot(slot, stack))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InventorySlot {
    WindowLocal(u16),
    MainInventory(u8),
    Hotbar(u8),
    Offhand,
}

impl InventorySlot {
    #[must_use]
    pub const fn crafting_result() -> Self {
        Self::WindowLocal(0)
    }

    #[must_use]
    pub const fn crafting_input(index: u8) -> Option<Self> {
        if index < 4 {
            Some(Self::WindowLocal((index + 1) as u16))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn container(index: u8) -> Self {
        Self::WindowLocal(index as u16)
    }

    #[must_use]
    pub(crate) const fn legacy_window_index(self) -> Option<u8> {
        match self {
            Self::WindowLocal(index) if index < AUXILIARY_SLOT_COUNT as u16 => Some(index as u8),
            Self::MainInventory(index) if index < MAIN_INVENTORY_SLOT_COUNT => {
                Some(AUXILIARY_SLOT_COUNT + index)
            }
            Self::Hotbar(index) if index < HOTBAR_SLOT_COUNT => Some(HOTBAR_START_SLOT + index),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_storage_slot(self) -> bool {
        matches!(
            self,
            Self::MainInventory(_) | Self::Hotbar(_) | Self::Offhand
        )
    }

    #[must_use]
    pub const fn container_index(self) -> Option<u8> {
        match self {
            Self::WindowLocal(index) if index <= u8::MAX as u16 => Some(index as u8),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_crafting_result(self) -> bool {
        matches!(self, Self::WindowLocal(0))
    }

    #[must_use]
    pub const fn crafting_input_index(self) -> Option<u8> {
        match self {
            Self::WindowLocal(index) if index >= 1 && index <= 4 => Some(index as u8 - 1),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_reserved_auxiliary(self) -> bool {
        matches!(self, Self::WindowLocal(5..=8))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryWindowContents {
    pub player_inventory: PlayerInventory,
    #[serde(default, alias = "container_slots")]
    pub local_slots: Vec<Option<ItemStack>>,
}

impl InventoryWindowContents {
    #[must_use]
    pub fn player(player_inventory: PlayerInventory) -> Self {
        Self {
            player_inventory,
            local_slots: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_local_slots(
        player_inventory: PlayerInventory,
        local_slots: Vec<Option<ItemStack>>,
    ) -> Self {
        Self {
            player_inventory,
            local_slots,
        }
    }

    #[must_use]
    pub fn with_container(
        player_inventory: PlayerInventory,
        container_slots: Vec<Option<ItemStack>>,
    ) -> Self {
        Self::with_local_slots(player_inventory, container_slots)
    }

    #[must_use]
    pub fn get_slot(&self, slot: InventorySlot) -> Option<&ItemStack> {
        match slot {
            InventorySlot::WindowLocal(index) => self
                .local_slots
                .get(usize::from(index))
                .and_then(std::option::Option::as_ref),
            _ => self.player_inventory.get_slot(slot),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InteractionHand {
    Main,
    Offhand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryTransactionContext {
    pub window_id: u8,
    pub action_number: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InventoryClickButton {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InventoryClickTarget {
    Slot(InventorySlot),
    Outside,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum InventoryClickValidation {
    StrictSlotEcho { clicked_item: Option<ItemStack> },
    Authoritative,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DroppedItemSnapshot {
    pub item: ItemStack,
    pub position: Vec3,
    pub velocity: Vec3,
}

fn floor_to_i32(value: f64) -> i32 {
    value
        .floor()
        .to_i32()
        .expect("world coordinate should fit into i32")
}

pub fn section_local_y(y: i32) -> u8 {
    u8::try_from(y.rem_euclid(SECTION_HEIGHT)).expect("section-local y should fit into u8")
}

pub fn required_chunks(center: ChunkPos, view_distance: u8) -> BTreeSet<ChunkPos> {
    let radius = i32::from(view_distance);
    let mut chunks = BTreeSet::new();
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            chunks.insert(ChunkPos::new(center.x + dx, center.z + dz));
        }
    }
    chunks
}

pub fn flatten_block_index(x: u8, y: u8, z: u8) -> SectionBlockIndex {
    SectionBlockIndex::new(x, y, z)
}

#[must_use]
pub const fn expand_block_index(index: u16) -> (u8, u8, u8) {
    SectionBlockIndex::from_raw(index).expand()
}
