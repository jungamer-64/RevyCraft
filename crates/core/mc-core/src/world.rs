use crate::inventory::ItemStack;
use crate::player::PlayerSnapshot;
use crate::{CHUNK_WIDTH, PlayerId, SECTION_HEIGHT};
use num_traits::ToPrimitive;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};

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

    #[must_use]
    pub fn air() -> Self {
        Self::new("minecraft:air")
    }

    #[must_use]
    pub fn bedrock() -> Self {
        Self::new("minecraft:bedrock")
    }

    #[must_use]
    pub fn stone() -> Self {
        Self::new("minecraft:stone")
    }

    #[must_use]
    pub fn dirt() -> Self {
        Self::new("minecraft:dirt")
    }

    #[must_use]
    pub fn grass_block() -> Self {
        Self::new("minecraft:grass_block")
    }

    #[must_use]
    pub fn cobblestone() -> Self {
        Self::new("minecraft:cobblestone")
    }

    #[must_use]
    pub fn oak_planks() -> Self {
        Self::new("minecraft:oak_planks")
    }

    #[must_use]
    pub fn sand() -> Self {
        Self::new("minecraft:sand")
    }

    #[must_use]
    pub fn sandstone() -> Self {
        Self::new("minecraft:sandstone")
    }

    #[must_use]
    pub fn glass() -> Self {
        Self::new("minecraft:glass")
    }

    #[must_use]
    pub fn bricks() -> Self {
        Self::new("minecraft:bricks")
    }

    #[must_use]
    pub fn crafting_table() -> Self {
        Self::new("minecraft:crafting_table")
    }

    #[must_use]
    pub fn chest() -> Self {
        Self::new("minecraft:chest")
    }

    #[must_use]
    pub fn furnace() -> Self {
        Self::new("minecraft:furnace")
    }

    #[must_use]
    pub fn is_air(&self) -> bool {
        self.key.as_str() == "minecraft:air"
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockEntityState {
    Chest {
        slots: Vec<Option<ItemStack>>,
    },
    Furnace {
        input: Option<ItemStack>,
        fuel: Option<ItemStack>,
        output: Option<ItemStack>,
        burn_left: i16,
        burn_max: i16,
        cook_progress: i16,
        cook_total: i16,
    },
}

impl BlockEntityState {
    #[must_use]
    pub fn chest(local_slot_count: usize) -> Self {
        Self::Chest {
            slots: vec![None; local_slot_count],
        }
    }

    #[must_use]
    pub const fn furnace() -> Self {
        Self::Furnace {
            input: None,
            fuel: None,
            output: None,
            burn_left: 0,
            burn_max: 0,
            cook_progress: 0,
            cook_total: 200,
        }
    }

    #[must_use]
    pub fn chest_slots(&self) -> Option<&[Option<ItemStack>]> {
        match self {
            Self::Chest { slots } => Some(slots),
            Self::Furnace { .. } => None,
        }
    }

    #[must_use]
    pub fn chest_slots_mut(&mut self) -> Option<&mut Vec<Option<ItemStack>>> {
        match self {
            Self::Chest { slots } => Some(slots),
            Self::Furnace { .. } => None,
        }
    }

    #[must_use]
    pub fn furnace_state(
        &self,
    ) -> Option<(
        Option<&ItemStack>,
        Option<&ItemStack>,
        Option<&ItemStack>,
        i16,
        i16,
        i16,
        i16,
    )> {
        match self {
            Self::Chest { .. } => None,
            Self::Furnace {
                input,
                fuel,
                output,
                burn_left,
                burn_max,
                cook_progress,
                cook_total,
            } => Some((
                input.as_ref(),
                fuel.as_ref(),
                output.as_ref(),
                *burn_left,
                *burn_max,
                *cook_progress,
                *cook_total,
            )),
        }
    }

    #[must_use]
    pub fn has_inventory_contents(&self) -> bool {
        match self {
            Self::Chest { slots } => slots.iter().any(Option::is_some),
            Self::Furnace {
                input,
                fuel,
                output,
                ..
            } => input.is_some() || fuel.is_some() || output.is_some(),
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

    pub fn set_block(&mut self, x: u8, y: u8, z: u8, state: BlockState) {
        let index = flatten_block_index(x, y, z);
        if state.is_air() {
            self.blocks.remove(&index);
        } else {
            self.blocks.insert(index, state);
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
    pub fn get_block(&self, x: u8, y: i32, z: u8) -> BlockState {
        let section_y = y.div_euclid(SECTION_HEIGHT);
        let local_y = section_local_y(y);
        self.sections
            .get(&section_y)
            .and_then(|section| section.get_block(x, local_y, z))
            .cloned()
            .unwrap_or_else(BlockState::air)
    }

    pub fn set_block(&mut self, x: u8, y: i32, z: u8, state: BlockState) {
        let section_y = y.div_euclid(SECTION_HEIGHT);
        let local_y = section_local_y(y);
        let section = self
            .sections
            .entry(section_y)
            .or_insert_with(|| ChunkSection::new(section_y));
        section.set_block(x, local_y, z, state);
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DroppedItemSnapshot {
    pub item: ItemStack,
    pub position: Vec3,
    pub velocity: Vec3,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub meta: WorldMeta,
    pub chunks: BTreeMap<ChunkPos, ChunkColumn>,
    #[serde(default)]
    pub block_entities: BTreeMap<BlockPos, BlockEntityState>,
    pub players: BTreeMap<PlayerId, PlayerSnapshot>,
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

pub fn generate_superflat_chunk(chunk_pos: ChunkPos) -> ChunkColumn {
    let mut column = ChunkColumn::new(chunk_pos);
    for z in 0..CHUNK_WIDTH {
        for x in 0..CHUNK_WIDTH {
            let x = u8::try_from(x).expect("flat chunk x should fit into u8");
            let z = u8::try_from(z).expect("flat chunk z should fit into u8");
            column.set_block(x, 0, z, BlockState::bedrock());
            column.set_block(x, 1, z, BlockState::stone());
            column.set_block(x, 2, z, BlockState::dirt());
            column.set_block(x, 3, z, BlockState::grass_block());
        }
    }
    column
}

pub fn flatten_block_index(x: u8, y: u8, z: u8) -> SectionBlockIndex {
    SectionBlockIndex::new(x, y, z)
}

#[must_use]
pub const fn expand_block_index(index: u16) -> (u8, u8, u8) {
    SectionBlockIndex::from_raw(index).expand()
}
