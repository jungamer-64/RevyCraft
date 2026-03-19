pub mod catalog;

use num_traits::ToPrimitive;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const CHUNK_WIDTH: i32 = 16;
const SECTION_HEIGHT: i32 = 16;
const DEFAULT_KEEPALIVE_INTERVAL_MS: u64 = 10_000;
const DEFAULT_KEEPALIVE_TIMEOUT_MS: u64 = 30_000;
const PLAYER_INVENTORY_SLOT_COUNT: usize = 45;
const AUXILIARY_SLOT_COUNT: u8 = 9;
const MAIN_INVENTORY_SLOT_COUNT: u8 = 27;
const HOTBAR_START_SLOT: u8 = 36;
const HOTBAR_SLOT_COUNT: u8 = 9;
const PLAYER_WIDTH: f64 = 0.6;
const PLAYER_HEIGHT: f64 = 1.8;
const BLOCK_EDIT_REACH: f64 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConnectionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId(pub i32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlayerId(pub Uuid);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PluginGenerationId(pub u64);

impl Serialize for PlayerId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.hyphenated().to_string())
    }
}

impl<'de> Deserialize<'de> for PlayerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let uuid = Uuid::parse_str(&value).map_err(serde::de::Error::custom)?;
        Ok(Self(uuid))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySet {
    capabilities: BTreeSet<String>,
}

impl CapabilitySet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, capability: impl Into<String>) -> bool {
        self.capabilities.insert(capability.into())
    }

    #[must_use]
    pub fn contains(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }

    #[must_use]
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.capabilities.iter().map(String::as_str)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GameplayProfileId(String);

impl GameplayProfileId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCapabilitySet {
    pub protocol: CapabilitySet,
    pub gameplay_profile: GameplayProfileId,
    pub plugin_generation: Option<PluginGenerationId>,
}

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
    pub fn is_air(&self) -> bool {
        self.key.as_str() == "minecraft:air"
    }
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

    #[must_use]
    pub fn is_supported_placeable(&self) -> bool {
        catalog::is_supported_placeable_item(self.key.as_str())
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
    pub fn creative_starter() -> Self {
        let mut inventory = Self::new_empty();
        for (slot, key) in (HOTBAR_START_SLOT..HOTBAR_START_SLOT + HOTBAR_SLOT_COUNT)
            .zip(catalog::starter_hotbar_item_keys())
        {
            let _ = inventory.set(slot, Some(ItemStack::new(key, 64, 0)));
        }
        inventory
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InventoryContainer {
    Player,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InventorySlot {
    Auxiliary(u8),
    MainInventory(u8),
    Hotbar(u8),
    Offhand,
}

impl InventorySlot {
    #[must_use]
    pub const fn legacy_window_index(self) -> Option<u8> {
        match self {
            Self::Auxiliary(index) if index < AUXILIARY_SLOT_COUNT => Some(index),
            Self::MainInventory(index) if index < MAIN_INVENTORY_SLOT_COUNT => {
                Some(AUXILIARY_SLOT_COUNT + index)
            }
            Self::Hotbar(index) if index < HOTBAR_SLOT_COUNT => Some(HOTBAR_START_SLOT + index),
            Self::Offhand => None,
            _ => None,
        }
    }

    #[must_use]
    pub const fn from_legacy_window_index(index: u8) -> Option<Self> {
        if index < AUXILIARY_SLOT_COUNT {
            Some(Self::Auxiliary(index))
        } else if index < HOTBAR_START_SLOT {
            Some(Self::MainInventory(index - AUXILIARY_SLOT_COUNT))
        } else if index < HOTBAR_START_SLOT + HOTBAR_SLOT_COUNT {
            Some(Self::Hotbar(index - HOTBAR_START_SLOT))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn is_storage_slot(self) -> bool {
        matches!(
            self,
            Self::MainInventory(_) | Self::Hotbar(_) | Self::Offhand
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InteractionHand {
    Main,
    Offhand,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkSection {
    pub y: i32,
    blocks: BTreeMap<u16, BlockState>,
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

    pub fn iter_blocks(&self) -> impl Iterator<Item = (u16, &BlockState)> {
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    pub id: PlayerId,
    pub username: String,
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub dimension: DimensionId,
    pub health: f32,
    pub food: i16,
    pub food_saturation: f32,
    pub inventory: PlayerInventory,
    pub selected_hotbar_slot: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientView {
    pub center: ChunkPos,
    pub view_distance: u8,
    pub loaded_chunks: BTreeSet<ChunkPos>,
}

impl ClientView {
    #[must_use]
    pub fn new(center: ChunkPos, view_distance: u8) -> Self {
        let loaded_chunks = required_chunks(center, view_distance);
        Self {
            center,
            view_distance,
            loaded_chunks,
        }
    }

    #[must_use]
    pub fn retarget(&mut self, center: ChunkPos, view_distance: u8) -> ChunkDelta {
        let next_loaded = required_chunks(center, view_distance);
        let added = next_loaded
            .difference(&self.loaded_chunks)
            .copied()
            .collect::<Vec<_>>();
        let removed = self
            .loaded_chunks
            .difference(&next_loaded)
            .copied()
            .collect::<Vec<_>>();
        self.center = center;
        self.view_distance = view_distance;
        self.loaded_chunks = next_loaded;
        ChunkDelta { added, removed }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkDelta {
    pub added: Vec<ChunkPos>,
    pub removed: Vec<ChunkPos>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub meta: WorldMeta,
    pub chunks: BTreeMap<ChunkPos, ChunkColumn>,
    pub players: BTreeMap<PlayerId, PlayerSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreConfig {
    pub level_name: String,
    pub seed: u64,
    pub max_players: u8,
    pub view_distance: u8,
    pub game_mode: u8,
    pub difficulty: u8,
    pub spawn: BlockPos,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            level_name: "world".to_string(),
            seed: 0,
            max_players: 20,
            view_distance: 2,
            game_mode: 0,
            difficulty: 1,
            spawn: BlockPos::new(0, 4, 0),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CoreCommand {
    LoginStart {
        connection_id: ConnectionId,
        username: String,
        player_id: PlayerId,
    },
    UpdateClientView {
        player_id: PlayerId,
        view_distance: u8,
    },
    ClientStatus {
        player_id: PlayerId,
        action_id: i8,
    },
    MoveIntent {
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    },
    KeepAliveResponse {
        player_id: PlayerId,
        keep_alive_id: i32,
    },
    SetHeldSlot {
        player_id: PlayerId,
        slot: i16,
    },
    CreativeInventorySet {
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    },
    DigBlock {
        player_id: PlayerId,
        position: BlockPos,
        status: u8,
        face: Option<BlockFace>,
    },
    PlaceBlock {
        player_id: PlayerId,
        hand: InteractionHand,
        position: BlockPos,
        face: Option<BlockFace>,
        held_item: Option<ItemStack>,
    },
    Disconnect {
        player_id: PlayerId,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CoreEvent {
    LoginAccepted {
        player_id: PlayerId,
        entity_id: EntityId,
        player: PlayerSnapshot,
    },
    PlayBootstrap {
        player: PlayerSnapshot,
        entity_id: EntityId,
        world_meta: WorldMeta,
        view_distance: u8,
    },
    ChunkBatch {
        chunks: Vec<ChunkColumn>,
    },
    EntitySpawned {
        entity_id: EntityId,
        player: PlayerSnapshot,
    },
    EntityMoved {
        entity_id: EntityId,
        player: PlayerSnapshot,
    },
    EntityDespawned {
        entity_ids: Vec<EntityId>,
    },
    InventoryContents {
        container: InventoryContainer,
        inventory: PlayerInventory,
    },
    InventorySlotChanged {
        container: InventoryContainer,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    },
    SelectedHotbarSlotChanged {
        slot: u8,
    },
    BlockChanged {
        position: BlockPos,
        block: BlockState,
    },
    KeepAliveRequested {
        keep_alive_id: i32,
    },
    Disconnect {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventTarget {
    Connection(ConnectionId),
    Player(PlayerId),
    EveryoneExcept(PlayerId),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetedEvent {
    pub target: EventTarget,
    pub event: CoreEvent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerSummary {
    pub online_players: usize,
    pub max_players: u8,
}

#[derive(Clone, Debug)]
pub struct ServerCore {
    config: CoreConfig,
    world_meta: WorldMeta,
    chunks: BTreeMap<ChunkPos, ChunkColumn>,
    saved_players: BTreeMap<PlayerId, PlayerSnapshot>,
    online_players: BTreeMap<PlayerId, OnlinePlayer>,
    next_entity_id: i32,
    next_keep_alive_id: i32,
    keepalive_interval_ms: u64,
    keepalive_timeout_ms: u64,
}

#[derive(Clone, Debug)]
struct OnlinePlayer {
    entity_id: EntityId,
    snapshot: PlayerSnapshot,
    view: ClientView,
    pending_keep_alive_id: Option<i32>,
    last_keep_alive_sent_at: Option<u64>,
    next_keep_alive_at: u64,
}

impl ServerCore {
    #[must_use]
    pub fn new(config: CoreConfig) -> Self {
        let world_meta = WorldMeta {
            level_name: config.level_name.clone(),
            seed: config.seed,
            spawn: config.spawn,
            dimension: DimensionId::Overworld,
            age: 0,
            time: 6000,
            level_type: "FLAT".to_string(),
            game_mode: config.game_mode,
            difficulty: config.difficulty,
            max_players: config.max_players,
        };
        Self {
            config,
            world_meta,
            chunks: BTreeMap::new(),
            saved_players: BTreeMap::new(),
            online_players: BTreeMap::new(),
            next_entity_id: 1,
            next_keep_alive_id: 1,
            keepalive_interval_ms: DEFAULT_KEEPALIVE_INTERVAL_MS,
            keepalive_timeout_ms: DEFAULT_KEEPALIVE_TIMEOUT_MS,
        }
    }

    #[must_use]
    pub fn from_snapshot(config: CoreConfig, snapshot: WorldSnapshot) -> Self {
        let mut core = Self::new(config);
        core.world_meta = snapshot.meta;
        core.chunks = snapshot.chunks;
        core.saved_players = snapshot.players;
        core
    }

    #[must_use]
    pub fn snapshot(&self) -> WorldSnapshot {
        let mut players = self.saved_players.clone();
        for (player_id, player) in &self.online_players {
            players.insert(*player_id, player.snapshot.clone());
        }
        WorldSnapshot {
            meta: self.world_meta.clone(),
            chunks: self.chunks.clone(),
            players,
        }
    }

    #[must_use]
    pub fn player_summary(&self) -> PlayerSummary {
        PlayerSummary {
            online_players: self.online_players.len(),
            max_players: self.config.max_players,
        }
    }

    #[must_use]
    pub const fn world_meta(&self) -> &WorldMeta {
        &self.world_meta
    }

    pub fn apply_command(&mut self, command: CoreCommand, now_ms: u64) -> Vec<TargetedEvent> {
        match command {
            CoreCommand::LoginStart {
                connection_id,
                username,
                player_id,
            } => self.login_player(connection_id, username, player_id, now_ms),
            CoreCommand::UpdateClientView {
                player_id,
                view_distance,
            } => self.update_client_settings(player_id, view_distance),
            CoreCommand::ClientStatus {
                player_id: _,
                action_id: _,
            } => Vec::new(),
            CoreCommand::MoveIntent {
                player_id,
                position,
                yaw,
                pitch,
                on_ground,
            } => self.apply_move(player_id, position, yaw, pitch, on_ground),
            CoreCommand::KeepAliveResponse {
                player_id,
                keep_alive_id,
            } => {
                self.accept_keep_alive(player_id, keep_alive_id);
                Vec::new()
            }
            CoreCommand::SetHeldSlot { player_id, slot } => self.set_held_slot(player_id, slot),
            CoreCommand::CreativeInventorySet {
                player_id,
                slot,
                stack,
            } => self.set_creative_inventory_slot(player_id, slot, stack),
            CoreCommand::DigBlock {
                player_id,
                position,
                status,
                face,
            } => self.apply_dig(player_id, position, status, face),
            CoreCommand::PlaceBlock {
                player_id,
                hand,
                position,
                face,
                held_item,
            } => self.apply_place(player_id, hand, position, face, held_item.as_ref()),
            CoreCommand::Disconnect { player_id } => self.disconnect_player(player_id),
        }
    }

    pub fn tick(&mut self, now_ms: u64) -> Vec<TargetedEvent> {
        let mut events = Vec::new();
        let player_ids = self.online_players.keys().copied().collect::<Vec<_>>();
        for player_id in player_ids {
            let Some(player) = self.online_players.get_mut(&player_id) else {
                continue;
            };
            if let Some(sent_at) = player.last_keep_alive_sent_at
                && now_ms.saturating_sub(sent_at) > self.keepalive_timeout_ms
            {
                events.extend(self.disconnect_player(player_id));
                continue;
            }
            if player.pending_keep_alive_id.is_none() && now_ms >= player.next_keep_alive_at {
                let keep_alive_id = self.next_keep_alive_id;
                self.next_keep_alive_id = self.next_keep_alive_id.saturating_add(1);
                player.pending_keep_alive_id = Some(keep_alive_id);
                player.last_keep_alive_sent_at = Some(now_ms);
                player.next_keep_alive_at = now_ms.saturating_add(self.keepalive_interval_ms);
                events.push(TargetedEvent {
                    target: EventTarget::Player(player_id),
                    event: CoreEvent::KeepAliveRequested { keep_alive_id },
                });
            }
        }
        events
    }

    fn login_player(
        &mut self,
        connection_id: ConnectionId,
        username: String,
        player_id: PlayerId,
        now_ms: u64,
    ) -> Vec<TargetedEvent> {
        if username.is_empty() || username.len() > 16 {
            return Self::reject_connection(connection_id, "Invalid username");
        }
        if self.online_players.len() >= usize::from(self.config.max_players) {
            return Self::reject_connection(connection_id, "Server is full");
        }
        if self.online_players.contains_key(&player_id) {
            return Self::reject_connection(connection_id, "Player is already online");
        }

        let mut player = self
            .saved_players
            .get(&player_id)
            .cloned()
            .unwrap_or_else(|| default_player(player_id, username.clone(), self.config.spawn));
        player.username = username;

        let entity_id = EntityId(self.next_entity_id);
        self.next_entity_id = self.next_entity_id.saturating_add(1);

        let existing_players = self
            .online_players
            .values()
            .map(|online| (online.entity_id, online.snapshot.clone()))
            .collect::<Vec<_>>();

        let visible_chunks =
            self.initial_visible_chunks(player.position.chunk_pos(), self.config.view_distance);
        let view = ClientView::new(player.position.chunk_pos(), self.config.view_distance);

        self.online_players.insert(
            player_id,
            OnlinePlayer {
                entity_id,
                snapshot: player.clone(),
                view,
                pending_keep_alive_id: None,
                last_keep_alive_sent_at: None,
                next_keep_alive_at: now_ms.saturating_add(self.keepalive_interval_ms),
            },
        );

        let mut events =
            self.login_initial_events(connection_id, player_id, entity_id, &player, visible_chunks);
        events.extend(Self::existing_player_spawn_events(
            connection_id,
            existing_players,
        ));

        events.push(TargetedEvent {
            target: EventTarget::EveryoneExcept(player_id),
            event: CoreEvent::EntitySpawned { entity_id, player },
        });
        events
    }

    fn reject_connection(connection_id: ConnectionId, reason: &str) -> Vec<TargetedEvent> {
        vec![TargetedEvent {
            target: EventTarget::Connection(connection_id),
            event: CoreEvent::Disconnect {
                reason: reason.to_string(),
            },
        }]
    }

    fn login_initial_events(
        &self,
        connection_id: ConnectionId,
        player_id: PlayerId,
        entity_id: EntityId,
        player: &PlayerSnapshot,
        visible_chunks: Vec<ChunkColumn>,
    ) -> Vec<TargetedEvent> {
        vec![
            TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::LoginAccepted {
                    player_id,
                    entity_id,
                    player: player.clone(),
                },
            },
            TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::PlayBootstrap {
                    player: player.clone(),
                    entity_id,
                    world_meta: self.world_meta.clone(),
                    view_distance: self.config.view_distance,
                },
            },
            TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::ChunkBatch {
                    chunks: visible_chunks,
                },
            },
            TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::InventoryContents {
                    container: InventoryContainer::Player,
                    inventory: player.inventory.clone(),
                },
            },
            TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::SelectedHotbarSlotChanged {
                    slot: player.selected_hotbar_slot,
                },
            },
        ]
    }

    fn existing_player_spawn_events(
        connection_id: ConnectionId,
        existing_players: Vec<(EntityId, PlayerSnapshot)>,
    ) -> Vec<TargetedEvent> {
        existing_players
            .into_iter()
            .map(|(entity_id, player)| TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::EntitySpawned { entity_id, player },
            })
            .collect()
    }

    fn update_client_settings(
        &mut self,
        player_id: PlayerId,
        view_distance: u8,
    ) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return Vec::new();
        };
        let capped_view_distance = view_distance.min(self.config.view_distance).max(1);
        let delta = player
            .view
            .retarget(player.snapshot.position.chunk_pos(), capped_view_distance);
        delta
            .added
            .into_iter()
            .map(|chunk_pos| TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::ChunkBatch {
                    chunks: vec![self.ensure_chunk(chunk_pos).clone()],
                },
            })
            .collect()
    }

    fn apply_move(
        &mut self,
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    ) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return Vec::new();
        };

        if let Some(position) = position {
            player.snapshot.position = position;
        }
        if let Some(yaw) = yaw {
            player.snapshot.yaw = yaw;
        }
        if let Some(pitch) = pitch {
            player.snapshot.pitch = pitch;
        }
        player.snapshot.on_ground = on_ground;

        let delta = player.view.retarget(
            player.snapshot.position.chunk_pos(),
            player.view.view_distance,
        );
        let snapshot = player.snapshot.clone();
        let entity_id = player.entity_id;
        let added_chunks = delta.added;
        self.saved_players.insert(player_id, snapshot.clone());

        let mut events = Vec::new();
        for chunk_pos in added_chunks {
            events.push(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::ChunkBatch {
                    chunks: vec![self.ensure_chunk(chunk_pos).clone()],
                },
            });
        }

        events.push(TargetedEvent {
            target: EventTarget::EveryoneExcept(player_id),
            event: CoreEvent::EntityMoved {
                entity_id,
                player: snapshot,
            },
        });
        events
    }

    fn accept_keep_alive(&mut self, player_id: PlayerId, keep_alive_id: i32) {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return;
        };
        if player.pending_keep_alive_id == Some(keep_alive_id) {
            player.pending_keep_alive_id = None;
            player.last_keep_alive_sent_at = None;
        }
    }

    fn set_held_slot(&mut self, player_id: PlayerId, slot: i16) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return Vec::new();
        };
        let slot = match u8::try_from(slot) {
            Ok(slot) if slot < HOTBAR_SLOT_COUNT => slot,
            _ => {
                return vec![TargetedEvent {
                    target: EventTarget::Player(player_id),
                    event: CoreEvent::SelectedHotbarSlotChanged {
                        slot: player.snapshot.selected_hotbar_slot,
                    },
                }];
            }
        };
        player.snapshot.selected_hotbar_slot = slot;
        self.saved_players
            .insert(player_id, player.snapshot.clone());
        vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::SelectedHotbarSlotChanged { slot },
        }]
    }

    fn set_creative_inventory_slot(
        &mut self,
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    ) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return Vec::new();
        };
        if self.world_meta.game_mode != 1 {
            return reject_inventory_slot_events(player_id, slot, player);
        }

        if !slot.is_storage_slot() || matches!(slot, InventorySlot::Auxiliary(_)) {
            return reject_inventory_slot_events(player_id, slot, player);
        }

        if let Some(stack) = stack.as_ref()
            && (!stack.is_supported_placeable() || stack.count == 0 || stack.count > 64)
        {
            return reject_inventory_slot_events(player_id, slot, player);
        }

        let _ = player.snapshot.inventory.set_slot(slot, stack.clone());
        self.saved_players
            .insert(player_id, player.snapshot.clone());
        vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::InventorySlotChanged {
                container: InventoryContainer::Player,
                slot,
                stack,
            },
        }]
    }

    fn apply_dig(
        &mut self,
        player_id: PlayerId,
        position: BlockPos,
        status: u8,
        _face: Option<BlockFace>,
    ) -> Vec<TargetedEvent> {
        if !matches!(status, 0 | 2) {
            return Vec::new();
        }
        let Some(player) = self.online_players.get(&player_id) else {
            return Vec::new();
        };
        if self.world_meta.game_mode != 1 || !self.can_edit_block(&player.snapshot, position) {
            let current = self.block_at(position);
            return vec![TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::BlockChanged {
                    position,
                    block: current,
                },
            }];
        }

        let current = self.block_at(position);
        if current.is_air() || current.key.as_str() == "minecraft:bedrock" {
            return vec![TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::BlockChanged {
                    position,
                    block: current,
                },
            }];
        }

        self.set_block_at(position, BlockState::air());
        self.emit_block_change(position)
    }

    fn apply_place(
        &mut self,
        player_id: PlayerId,
        hand: InteractionHand,
        position: BlockPos,
        face: Option<BlockFace>,
        held_item: Option<&ItemStack>,
    ) -> Vec<TargetedEvent> {
        let Some(face) = face else {
            return Vec::new();
        };
        let Some(player) = self.online_players.get(&player_id) else {
            return Vec::new();
        };
        let place_pos = position.offset(face);
        let Some(selected_stack) = player
            .snapshot
            .inventory
            .selected_stack(player.snapshot.selected_hotbar_slot, hand)
            .cloned()
        else {
            return self.place_rejection_events(player_id, hand, place_pos, &player.snapshot);
        };

        if held_item.is_some() && held_item != Some(&selected_stack) {
            return self.place_rejection_events(player_id, hand, place_pos, &player.snapshot);
        }

        let Some(block) = catalog::placeable_block_state_from_item_key(selected_stack.key.as_str())
        else {
            return self.place_rejection_events(player_id, hand, place_pos, &player.snapshot);
        };

        if self.world_meta.game_mode != 1
            || !self.can_edit_block(&player.snapshot, place_pos)
            || self.block_at(position).is_air()
            || !self.block_at(place_pos).is_air()
        {
            return self.place_rejection_events(player_id, hand, place_pos, &player.snapshot);
        }

        self.set_block_at(place_pos, block);
        self.emit_block_change(place_pos)
    }

    fn place_inventory_correction(
        player_id: PlayerId,
        hand: InteractionHand,
        player: &PlayerSnapshot,
    ) -> Vec<TargetedEvent> {
        let selected_slot = match hand {
            InteractionHand::Main => InventorySlot::Hotbar(player.selected_hotbar_slot),
            InteractionHand::Offhand => InventorySlot::Offhand,
        };
        vec![
            TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventorySlotChanged {
                    container: InventoryContainer::Player,
                    slot: selected_slot,
                    stack: player.inventory.get_slot(selected_slot).cloned(),
                },
            },
            TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::SelectedHotbarSlotChanged {
                    slot: player.selected_hotbar_slot,
                },
            },
        ]
    }

    fn place_rejection_events(
        &self,
        player_id: PlayerId,
        hand: InteractionHand,
        place_pos: BlockPos,
        player: &PlayerSnapshot,
    ) -> Vec<TargetedEvent> {
        let mut events = vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::BlockChanged {
                position: place_pos,
                block: self.block_at(place_pos),
            },
        }];
        events.extend(Self::place_inventory_correction(player_id, hand, player));
        events
    }

    fn disconnect_player(&mut self, player_id: PlayerId) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.remove(&player_id) else {
            return Vec::new();
        };
        self.saved_players.insert(player_id, player.snapshot);
        vec![TargetedEvent {
            target: EventTarget::EveryoneExcept(player_id),
            event: CoreEvent::EntityDespawned {
                entity_ids: vec![player.entity_id],
            },
        }]
    }

    fn initial_visible_chunks(&mut self, center: ChunkPos, view_distance: u8) -> Vec<ChunkColumn> {
        required_chunks(center, view_distance)
            .into_iter()
            .map(|chunk_pos| self.ensure_chunk(chunk_pos).clone())
            .collect()
    }

    fn ensure_chunk(&mut self, chunk_pos: ChunkPos) -> &ChunkColumn {
        self.chunks
            .entry(chunk_pos)
            .or_insert_with(|| generate_superflat_chunk(chunk_pos))
    }

    fn block_at(&self, position: BlockPos) -> BlockState {
        let chunk_pos = position.chunk_pos();
        let local_x =
            u8::try_from(position.x.rem_euclid(CHUNK_WIDTH)).expect("local x should fit into u8");
        let local_z =
            u8::try_from(position.z.rem_euclid(CHUNK_WIDTH)).expect("local z should fit into u8");
        self.chunks
            .get(&chunk_pos)
            .cloned()
            .unwrap_or_else(|| generate_superflat_chunk(chunk_pos))
            .get_block(local_x, position.y, local_z)
    }

    fn set_block_at(&mut self, position: BlockPos, state: BlockState) {
        let chunk_pos = position.chunk_pos();
        let local_x =
            u8::try_from(position.x.rem_euclid(CHUNK_WIDTH)).expect("local x should fit into u8");
        let local_z =
            u8::try_from(position.z.rem_euclid(CHUNK_WIDTH)).expect("local z should fit into u8");
        let chunk = self
            .chunks
            .entry(chunk_pos)
            .or_insert_with(|| generate_superflat_chunk(chunk_pos));
        chunk.set_block(local_x, position.y, local_z, state);
    }

    fn emit_block_change(&self, position: BlockPos) -> Vec<TargetedEvent> {
        let block = self.block_at(position);
        self.online_players
            .iter()
            .filter(|(_, player)| player.view.loaded_chunks.contains(&position.chunk_pos()))
            .map(|(player_id, _)| TargetedEvent {
                target: EventTarget::Player(*player_id),
                event: CoreEvent::BlockChanged {
                    position,
                    block: block.clone(),
                },
            })
            .collect()
    }

    fn can_edit_block(&self, actor: &PlayerSnapshot, position: BlockPos) -> bool {
        if !(0..=255).contains(&position.y) {
            return false;
        }
        if distance_squared_to_block_center(actor.position, position) > BLOCK_EDIT_REACH.powi(2) {
            return false;
        }
        !self
            .online_players
            .iter()
            .any(|(_, player)| block_intersects_player(position, &player.snapshot))
    }
}

fn default_player(player_id: PlayerId, username: String, spawn: BlockPos) -> PlayerSnapshot {
    PlayerSnapshot {
        id: player_id,
        username,
        position: Vec3::new(
            f64::from(spawn.x) + 0.5,
            f64::from(spawn.y),
            f64::from(spawn.z) + 0.5,
        ),
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        dimension: DimensionId::Overworld,
        health: 20.0,
        food: 20,
        food_saturation: 5.0,
        inventory: PlayerInventory::creative_starter(),
        selected_hotbar_slot: 0,
    }
}

fn distance_squared_to_block_center(position: Vec3, block: BlockPos) -> f64 {
    let eye_x = position.x;
    let eye_y = position.y + 1.62;
    let eye_z = position.z;
    let center_x = f64::from(block.x) + 0.5;
    let center_y = f64::from(block.y) + 0.5;
    let center_z = f64::from(block.z) + 0.5;
    let dx = eye_x - center_x;
    let dy = eye_y - center_y;
    let dz = eye_z - center_z;
    dx * dx + dy * dy + dz * dz
}

fn floor_to_i32(value: f64) -> i32 {
    value
        .floor()
        .to_i32()
        .expect("world coordinate should fit into i32")
}

fn section_local_y(y: i32) -> u8 {
    u8::try_from(y.rem_euclid(SECTION_HEIGHT)).expect("section-local y should fit into u8")
}

fn block_intersects_player(block: BlockPos, player: &PlayerSnapshot) -> bool {
    let half_width = PLAYER_WIDTH / 2.0;
    let player_min_x = player.position.x - half_width;
    let player_max_x = player.position.x + half_width;
    let player_min_y = player.position.y;
    let player_max_y = player.position.y + PLAYER_HEIGHT;
    let player_min_z = player.position.z - half_width;
    let player_max_z = player.position.z + half_width;

    let block_min_x = f64::from(block.x);
    let block_max_x = block_min_x + 1.0;
    let block_min_y = f64::from(block.y);
    let block_max_y = block_min_y + 1.0;
    let block_min_z = f64::from(block.z);
    let block_max_z = block_min_z + 1.0;

    player_min_x < block_max_x
        && player_max_x > block_min_x
        && player_min_y < block_max_y
        && player_max_y > block_min_y
        && player_min_z < block_max_z
        && player_max_z > block_min_z
}

fn reject_inventory_slot_events(
    player_id: PlayerId,
    slot: InventorySlot,
    player: &OnlinePlayer,
) -> Vec<TargetedEvent> {
    let mut events = vec![TargetedEvent {
        target: EventTarget::Player(player_id),
        event: CoreEvent::InventorySlotChanged {
            container: InventoryContainer::Player,
            slot,
            stack: player.snapshot.inventory.get_slot(slot).cloned(),
        },
    }];
    events.push(TargetedEvent {
        target: EventTarget::Player(player_id),
        event: CoreEvent::SelectedHotbarSlotChanged {
            slot: player.snapshot.selected_hotbar_slot,
        },
    });
    events
}

fn required_chunks(center: ChunkPos, view_distance: u8) -> BTreeSet<ChunkPos> {
    let radius = i32::from(view_distance);
    let mut chunks = BTreeSet::new();
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            chunks.insert(ChunkPos::new(center.x + dx, center.z + dz));
        }
    }
    chunks
}

fn generate_superflat_chunk(chunk_pos: ChunkPos) -> ChunkColumn {
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

fn flatten_block_index(x: u8, y: u8, z: u8) -> u16 {
    u16::from(y) * 256 + u16::from(z) * 16 + u16::from(x)
}

#[must_use]
pub const fn expand_block_index(index: u16) -> (u8, u8, u8) {
    let [y, lower] = index.to_be_bytes();
    let z = lower >> 4;
    let x = lower & 0x0F;
    (x, y, z)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player_id(name: &str) -> PlayerId {
        PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, name.as_bytes()))
    }

    #[test]
    fn chunk_column_stores_semantic_states() {
        let mut column = ChunkColumn::new(ChunkPos::new(0, 0));
        column.set_block(1, 12, 2, BlockState::grass_block());
        assert_eq!(
            column.get_block(1, 12, 2).key.as_str(),
            "minecraft:grass_block"
        );
        assert!(column.get_block(1, 32, 2).is_air());
    }

    #[test]
    fn login_emits_initial_chunks_and_existing_entities() {
        let mut core = ServerCore::new(CoreConfig {
            view_distance: 1,
            ..CoreConfig::default()
        });

        let first = player_id("first");
        let events = core.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(1),
                username: "first".to_string(),
                player_id: first,
            },
            0,
        );
        assert!(events.iter().any(|event| matches!(
            event.event,
            CoreEvent::PlayBootstrap {
                view_distance: 1,
                ..
            }
        )));
        assert!(events.iter().any(|event| {
            matches!(event.event, CoreEvent::ChunkBatch { ref chunks } if chunks.len() == 9)
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                TargetedEvent {
                    target: EventTarget::Connection(ConnectionId(1)),
                    event: CoreEvent::InventoryContents {
                        container: InventoryContainer::Player,
                        ..
                    },
                }
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                TargetedEvent {
                    target: EventTarget::Connection(ConnectionId(1)),
                    event: CoreEvent::SelectedHotbarSlotChanged { slot: 0 },
                }
            )
        }));

        let second = player_id("second");
        let events = core.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(2),
                username: "second".to_string(),
                player_id: second,
            },
            0,
        );
        assert!(events.iter().any(|event| {
            matches!(
                event,
                TargetedEvent {
                    target: EventTarget::Connection(ConnectionId(2)),
                    event: CoreEvent::EntitySpawned { .. },
                }
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                TargetedEvent {
                    target: EventTarget::EveryoneExcept(id),
                    event: CoreEvent::EntitySpawned { .. },
                } if *id == second
            )
        }));
    }

    #[test]
    fn moving_player_updates_other_clients_and_view() {
        let mut core = ServerCore::new(CoreConfig {
            view_distance: 1,
            ..CoreConfig::default()
        });
        let first = player_id("first");
        let second = player_id("second");
        let _ = core.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(1),
                username: "first".to_string(),
                player_id: first,
            },
            0,
        );
        let _ = core.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(2),
                username: "second".to_string(),
                player_id: second,
            },
            0,
        );

        let events = core.apply_command(
            CoreCommand::MoveIntent {
                player_id: second,
                position: Some(Vec3::new(32.5, 4.0, 0.5)),
                yaw: Some(90.0),
                pitch: Some(0.0),
                on_ground: true,
            },
            50,
        );

        assert!(events.iter().any(|event| {
            matches!(
                event,
                TargetedEvent {
                    target: EventTarget::EveryoneExcept(id),
                    event: CoreEvent::EntityMoved { .. },
                } if *id == second
            )
        }));
        assert!(
            events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        TargetedEvent {
                            target: EventTarget::Player(id),
                            event: CoreEvent::ChunkBatch { .. },
                        } if *id == second
                    )
                })
                .count()
                >= 3
        );
    }

    #[test]
    fn keepalive_tick_emits_keepalive() {
        let mut core = ServerCore::new(CoreConfig::default());
        let first = player_id("first");
        let _ = core.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(1),
                username: "first".to_string(),
                player_id: first,
            },
            0,
        );
        let events = core.tick(DEFAULT_KEEPALIVE_INTERVAL_MS + 1);
        assert!(events.iter().any(|event| matches!(
            event,
            TargetedEvent {
                target: EventTarget::Player(id),
                event: CoreEvent::KeepAliveRequested { .. },
            } if *id == first
        )));
    }

    #[test]
    fn world_snapshot_roundtrip_uses_semantic_types() {
        let mut core = ServerCore::new(CoreConfig::default());
        let first = player_id("first");
        let _ = core.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(1),
                username: "first".to_string(),
                player_id: first,
            },
            0,
        );
        let snapshot = core.snapshot();
        let json = serde_json::to_string(&snapshot).expect("snapshot should serialize");
        let decoded: WorldSnapshot =
            serde_json::from_str(&json).expect("snapshot should deserialize");
        assert_eq!(decoded.meta.level_type, "FLAT");
        assert!(
            decoded
                .chunks
                .values()
                .next()
                .expect("generated chunk should exist")
                .get_block(0, 3, 0)
                .key
                .as_str()
                == "minecraft:grass_block"
        );
        let player = decoded
            .players
            .get(&first)
            .expect("logged in player should persist");
        assert_eq!(player.selected_hotbar_slot, 0);
        assert_eq!(
            player
                .inventory
                .get(36)
                .expect("starter slot 36 should exist")
                .key
                .as_str(),
            "minecraft:stone"
        );
    }

    #[test]
    fn inventory_commands_update_selected_slot_and_slots() {
        let mut core = ServerCore::new(CoreConfig {
            game_mode: 1,
            ..CoreConfig::default()
        });
        let first = player_id("first");
        let _ = core.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(1),
                username: "first".to_string(),
                player_id: first,
            },
            0,
        );

        let slot_events = core.apply_command(
            CoreCommand::CreativeInventorySet {
                player_id: first,
                slot: InventorySlot::Hotbar(0),
                stack: Some(ItemStack::new("minecraft:glass", 64, 0)),
            },
            0,
        );
        assert!(slot_events.iter().any(|event| {
            matches!(
                event,
                TargetedEvent {
                    target: EventTarget::Player(id),
                    event: CoreEvent::InventorySlotChanged {
                        container: InventoryContainer::Player,
                        slot: InventorySlot::Hotbar(0),
                        ..
                    },
                } if *id == first
            )
        }));

        let held_events = core.apply_command(
            CoreCommand::SetHeldSlot {
                player_id: first,
                slot: 4,
            },
            0,
        );
        assert!(held_events.iter().any(|event| {
            matches!(
                event,
                TargetedEvent {
                    target: EventTarget::Player(id),
                    event: CoreEvent::SelectedHotbarSlotChanged { slot: 4 },
                } if *id == first
            )
        }));

        let snapshot = core.snapshot();
        let player = snapshot.players.get(&first).expect("player should persist");
        assert_eq!(player.selected_hotbar_slot, 4);
        assert_eq!(
            player
                .inventory
                .get_slot(InventorySlot::Hotbar(0))
                .expect("slot should be updated")
                .key
                .as_str(),
            "minecraft:glass"
        );
    }

    #[test]
    fn update_client_view_clamps_to_server_distance() {
        let mut core = ServerCore::new(CoreConfig {
            view_distance: 2,
            ..CoreConfig::default()
        });
        let first = player_id("first");
        let _ = core.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(1),
                username: "first".to_string(),
                player_id: first,
            },
            0,
        );

        let _ = core.apply_command(
            CoreCommand::UpdateClientView {
                player_id: first,
                view_distance: 1,
            },
            0,
        );

        let events = core.apply_command(
            CoreCommand::UpdateClientView {
                player_id: first,
                view_distance: 8,
            },
            0,
        );

        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        TargetedEvent {
                            target: EventTarget::Player(id),
                            event: CoreEvent::ChunkBatch { chunks },
                        } if *id == first && chunks.len() == 1
                    )
                })
                .count(),
            16
        );
    }

    #[test]
    fn creative_place_and_break_emit_authoritative_corrections() {
        let mut creative = ServerCore::new(CoreConfig {
            game_mode: 1,
            ..CoreConfig::default()
        });
        let first = player_id("first");
        let second = player_id("second");
        let _ = creative.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(1),
                username: "first".to_string(),
                player_id: first,
            },
            0,
        );
        let _ = creative.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(2),
                username: "second".to_string(),
                player_id: second,
            },
            0,
        );

        let place_events = creative.apply_command(
            CoreCommand::PlaceBlock {
                player_id: first,
                hand: InteractionHand::Main,
                position: BlockPos::new(2, 3, 0),
                face: Some(BlockFace::Top),
                held_item: Some(ItemStack::new("minecraft:stone", 64, 0)),
            },
            0,
        );
        assert!(
            place_events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    CoreEvent::BlockChanged {
                        position: BlockPos { x: 2, y: 4, z: 0 },
                        ..
                    }
                ))
                .count()
                >= 2
        );

        let break_events = creative.apply_command(
            CoreCommand::DigBlock {
                player_id: first,
                position: BlockPos::new(2, 4, 0),
                status: 0,
                face: Some(BlockFace::Top),
            },
            0,
        );
        assert!(
            break_events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    CoreEvent::BlockChanged {
                        position: BlockPos { x: 2, y: 4, z: 0 },
                        ref block,
                    } if block.is_air()
                ))
                .count()
                >= 2
        );
    }

    #[test]
    fn survival_place_rejection_emits_authoritative_corrections() {
        let mut survival = ServerCore::new(CoreConfig::default());
        let lone = player_id("lone");
        let _ = survival.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(3),
                username: "lone".to_string(),
                player_id: lone,
            },
            0,
        );
        let reject_events = survival.apply_command(
            CoreCommand::PlaceBlock {
                player_id: lone,
                hand: InteractionHand::Main,
                position: BlockPos::new(2, 3, 0),
                face: Some(BlockFace::Top),
                held_item: Some(ItemStack::new("minecraft:stone", 64, 0)),
            },
            0,
        );
        assert!(reject_events.iter().any(|event| matches!(
            event,
            TargetedEvent {
                target: EventTarget::Player(id),
                event: CoreEvent::BlockChanged {
                    position: BlockPos { x: 2, y: 4, z: 0 },
                    block,
                },
            } if *id == lone && block.is_air()
        )));
        assert!(reject_events.iter().any(|event| matches!(
            event,
            TargetedEvent {
                target: EventTarget::Player(id),
                event: CoreEvent::InventorySlotChanged {
                    container: InventoryContainer::Player,
                    slot: InventorySlot::Hotbar(0),
                    ..
                },
            } if *id == lone
        )));
    }
}
