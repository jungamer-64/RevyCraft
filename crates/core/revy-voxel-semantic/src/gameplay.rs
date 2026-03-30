use crate::{PlayerId, PlayerSnapshot, TargetedEvent};
use revy_voxel_model::{BlockPos, BlockState, InventorySlot, ItemStack, Vec3, WorldMeta};
use revy_voxel_rules::{BlockEntityState, ContainerKindId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GameplayCanEditBlockKey {
    pub player_id: PlayerId,
    pub position: BlockPos,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameplayReadSet {
    pub world_meta: Option<WorldMeta>,
    pub player_snapshots: BTreeMap<PlayerId, Option<PlayerSnapshot>>,
    pub block_states: BTreeMap<BlockPos, Option<BlockState>>,
    pub block_entities: BTreeMap<BlockPos, Option<BlockEntityState>>,
    pub can_edit_block: BTreeMap<GameplayCanEditBlockKey, bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GameplayEffect {
    SetPlayerPose {
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    },
    SetSelectedHotbarSlot {
        player_id: PlayerId,
        slot: u8,
    },
    SetInventorySlot {
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    },
    ClearMining {
        player_id: PlayerId,
    },
    BeginMining {
        player_id: PlayerId,
        position: BlockPos,
        duration_ms: u64,
    },
    OpenContainerAt {
        player_id: PlayerId,
        position: BlockPos,
    },
    OpenVirtualContainer {
        player_id: PlayerId,
        kind: ContainerKindId,
    },
    SetBlock {
        position: BlockPos,
        block: Option<BlockState>,
    },
    SpawnDroppedItem {
        position: Vec3,
        item: ItemStack,
    },
    EmitEvent {
        event: TargetedEvent,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameplayEffectBatch {
    pub now_ms: u64,
    pub reads: GameplayReadSet,
    pub effects: Vec<GameplayEffect>,
}

impl GameplayEffectBatch {
    #[must_use]
    pub fn empty(now_ms: u64) -> Self {
        Self {
            now_ms,
            reads: GameplayReadSet::default(),
            effects: Vec::new(),
        }
    }
}
