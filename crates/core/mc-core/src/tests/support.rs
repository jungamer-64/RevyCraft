pub(super) use crate::*;

use mc_content_canonical::catalog;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

const PLAYER_KIND: &str = "canonical:player";
const CRAFTING_TABLE_KIND: &str = "canonical:crafting_table";
const CHEST_27_KIND: &str = "canonical:chest_27";
const FURNACE_KIND: &str = "canonical:furnace";
const CHEST_BLOCK_ENTITY_KIND: &str = "canonical:chest";
const FURNACE_BLOCK_ENTITY_KIND: &str = "canonical:furnace";
const FURNACE_BURN_LEFT: &str = "canonical:furnace.burn_left";
const FURNACE_BURN_MAX: &str = "canonical:furnace.burn_max";
const FURNACE_COOK_PROGRESS: &str = "canonical:furnace.cook_progress";
const FURNACE_COOK_TOTAL: &str = "canonical:furnace.cook_total";
const PLAYER_LOCAL_SLOT_COUNT: u16 = 9;
const FURNACE_COOK_TOTAL_DEFAULT: i16 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InventoryContainer {
    Player,
    CraftingTable,
    Chest,
    Furnace,
}

#[derive(Clone, Debug, Default)]
struct TestContentBehavior;

pub(super) fn test_content_behavior() -> Arc<dyn mc_content_api::ContentBehavior> {
    Arc::new(TestContentBehavior)
}

pub(super) fn new_server_core(config: CoreConfig) -> ServerCore {
    ServerCore::new(config, test_content_behavior())
}

pub(super) fn restore_server_core_from_snapshot(
    config: CoreConfig,
    snapshot: WorldSnapshot,
) -> ServerCore {
    ServerCore::from_snapshot(config, snapshot, test_content_behavior())
}

pub(super) fn restore_server_core_from_runtime_state(
    config: CoreConfig,
    blob: CoreRuntimeStateBlob,
) -> ServerCore {
    ServerCore::from_runtime_state(config, blob, test_content_behavior())
}

pub(super) const fn container_slot(index: u8) -> InventorySlot {
    InventorySlot::container(index)
}

pub(super) const fn player_window_local(index: u8) -> InventorySlot {
    InventorySlot::WindowLocal(index as u16)
}

pub(super) fn container_kind(container: InventoryContainer) -> ContainerKindId {
    match container {
        InventoryContainer::Player => ContainerKindId::new(PLAYER_KIND),
        InventoryContainer::CraftingTable => ContainerKindId::new(CRAFTING_TABLE_KIND),
        InventoryContainer::Chest => ContainerKindId::new(CHEST_27_KIND),
        InventoryContainer::Furnace => ContainerKindId::new(FURNACE_KIND),
    }
}

pub(super) fn container_from_kind(kind: &ContainerKindId) -> Option<InventoryContainer> {
    match kind.as_str() {
        PLAYER_KIND => Some(InventoryContainer::Player),
        CRAFTING_TABLE_KIND => Some(InventoryContainer::CraftingTable),
        CHEST_27_KIND => Some(InventoryContainer::Chest),
        FURNACE_KIND => Some(InventoryContainer::Furnace),
        _ => None,
    }
}

pub(super) fn furnace_property_key(property_id: u8) -> ContainerPropertyKey {
    match property_id {
        0 => ContainerPropertyKey::new(FURNACE_BURN_LEFT),
        1 => ContainerPropertyKey::new(FURNACE_BURN_MAX),
        2 => ContainerPropertyKey::new(FURNACE_COOK_PROGRESS),
        3 => ContainerPropertyKey::new(FURNACE_COOK_TOTAL),
        _ => panic!("unsupported furnace property id {property_id}"),
    }
}

pub(super) fn crafting_table_state(
    window: &crate::core::OpenInventoryWindow,
) -> &mc_content_api::OpenContainerState {
    assert_eq!(
        window.container.kind,
        container_kind(InventoryContainer::CraftingTable)
    );
    &window.container
}

pub(super) fn chest_state(
    window: &crate::core::OpenInventoryWindow,
) -> &mc_content_api::OpenContainerState {
    assert_eq!(
        window.container.kind,
        container_kind(InventoryContainer::Chest)
    );
    &window.container
}

pub(super) fn chest_state_mut(
    window: &mut crate::core::OpenInventoryWindow,
) -> &mut mc_content_api::OpenContainerState {
    assert_eq!(
        window.container.kind,
        container_kind(InventoryContainer::Chest)
    );
    &mut window.container
}

pub(super) fn furnace_state(
    window: &crate::core::OpenInventoryWindow,
) -> &mc_content_api::OpenContainerState {
    assert_eq!(
        window.container.kind,
        container_kind(InventoryContainer::Furnace)
    );
    &window.container
}

pub(super) fn furnace_state_mut(
    window: &mut crate::core::OpenInventoryWindow,
) -> &mut mc_content_api::OpenContainerState {
    assert_eq!(
        window.container.kind,
        container_kind(InventoryContainer::Furnace)
    );
    &mut window.container
}

pub(super) fn furnace_block_entity(
    input: Option<ItemStack>,
    fuel: Option<ItemStack>,
    output: Option<ItemStack>,
    burn_left: i16,
    burn_max: i16,
    cook_progress: i16,
    cook_total: i16,
) -> BlockEntityState {
    BlockEntityState::Container(ContainerBlockEntityState {
        kind: BlockEntityKindId::new(FURNACE_BLOCK_ENTITY_KIND),
        slots: vec![input, fuel, output],
        properties: BTreeMap::from([
            (furnace_property_key(0), burn_left),
            (furnace_property_key(1), burn_max),
            (furnace_property_key(2), cook_progress),
            (furnace_property_key(3), cook_total),
        ]),
    })
}

pub(super) fn default_furnace_block_entity() -> BlockEntityState {
    furnace_block_entity(None, None, None, 0, 0, 0, 200)
}

pub(super) fn block_entity_slots(block_entity: &BlockEntityState) -> Option<&[Option<ItemStack>]> {
    block_entity
        .container_state()
        .map(|container| container.slots.as_slice())
}

pub(super) fn insert_container_viewer(
    core: &mut ServerCore,
    position: BlockPos,
    container: InventoryContainer,
    viewer: PlayerId,
    window_id: u8,
) {
    core.world
        .container_viewers
        .entry(position)
        .or_insert_with(|| WorldContainerViewers {
            kind: container_kind(container),
            viewers: BTreeMap::new(),
        })
        .viewers
        .insert(viewer, window_id);
}

pub(super) fn container_has_viewer(
    core: &ServerCore,
    position: BlockPos,
    viewer: PlayerId,
) -> bool {
    core.world
        .container_viewers
        .get(&position)
        .is_some_and(|entry| entry.viewers.contains_key(&viewer))
}

pub(super) fn player_id(name: &str) -> PlayerId {
    PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, name.as_bytes()))
}

pub(super) fn item(key: &str, count: u8) -> ItemStack {
    ItemStack::new(key, count, 0)
}

pub(super) fn login_player(
    core: &mut ServerCore,
    connection_id: u64,
    name: &str,
) -> (PlayerId, Vec<TargetedEvent>) {
    let player_id = player_id(name);
    let events = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(connection_id),
            username: name.to_string(),
            player_id,
        },
        0,
    );
    (player_id, events)
}

pub(super) fn logged_in_core(
    config: CoreConfig,
    connection_id: u64,
    name: &str,
) -> (ServerCore, PlayerId) {
    let mut core = new_server_core(config);
    let (player_id, _) = login_player(&mut core, connection_id, name);
    (core, player_id)
}

pub(super) fn logged_in_creative_core(name: &str) -> (ServerCore, PlayerId) {
    logged_in_core(
        CoreConfig {
            game_mode: 1,
            ..CoreConfig::default()
        },
        1,
        name,
    )
}

pub(super) fn creative_inventory_set(
    core: &mut ServerCore,
    player_id: PlayerId,
    slot: InventorySlot,
    stack: Option<ItemStack>,
) -> Vec<TargetedEvent> {
    core.apply_command(
        CoreCommand::CreativeInventorySet {
            player_id,
            slot,
            stack,
        },
        0,
    )
}

pub(super) fn set_held_slot(
    core: &mut ServerCore,
    player_id: PlayerId,
    slot: i16,
) -> Vec<TargetedEvent> {
    core.apply_command(CoreCommand::SetHeldSlot { player_id, slot }, 0)
}

pub(super) fn click_slot(
    core: &mut ServerCore,
    player_id: PlayerId,
    window_id: u8,
    action_number: i16,
    slot: InventorySlot,
    button: InventoryClickButton,
    clicked_item: Option<ItemStack>,
) -> Vec<TargetedEvent> {
    core.apply_command(
        CoreCommand::InventoryClick {
            player_id,
            transaction: InventoryTransactionContext {
                window_id,
                action_number,
            },
            target: InventoryClickTarget::Slot(slot),
            button,
            validation: InventoryClickValidation::StrictSlotEcho { clicked_item },
        },
        0,
    )
}

pub(super) fn apply_test_transaction(
    core: &mut ServerCore,
    now_ms: u64,
    f: impl FnOnce(&mut GameplayTransaction<'_>),
) -> Vec<TargetedEvent> {
    let mut tx = core.begin_gameplay_transaction(now_ms);
    f(&mut tx);
    tx.commit()
}

pub(super) fn open_virtual_container_for_test(
    core: &mut ServerCore,
    player_id: PlayerId,
    window_id: u8,
    container: InventoryContainer,
    now_ms: u64,
) -> Vec<TargetedEvent> {
    core.player_session_mut(player_id)
        .expect("player session should exist")
        .next_non_player_window_id = window_id;
    apply_test_transaction(core, now_ms, |tx| {
        tx.open_virtual_container(player_id, container_kind(container));
    })
}

pub(super) fn set_block_via_tx(
    core: &mut ServerCore,
    position: BlockPos,
    block: BlockState,
    now_ms: u64,
) -> Vec<TargetedEvent> {
    apply_test_transaction(core, now_ms, |tx| tx.set_block(position, Some(block)))
}

pub(super) fn spawn_dropped_item_via_tx(
    core: &mut ServerCore,
    position: Vec3,
    item: ItemStack,
    now_ms: u64,
) -> Vec<TargetedEvent> {
    apply_test_transaction(core, now_ms, |tx| tx.spawn_dropped_item(position, item))
}

pub(super) fn craft_input(index: u8) -> InventorySlot {
    InventorySlot::crafting_input(index).expect("craft input should exist")
}

pub(super) fn survival_mining_duration_ms_for_item(
    block: &BlockState,
    item: Option<&ItemStack>,
) -> Option<u64> {
    let behavior = test_content_behavior();
    let tool = behavior.tool_spec_for_item(item);
    behavior.survival_mining_duration_ms(block, tool)
}

fn creative_starter_inventory() -> PlayerInventory {
    let mut inventory = PlayerInventory::new_empty();
    for (slot, key) in (36_u8..45).zip([
        catalog::STONE,
        catalog::DIRT,
        catalog::GRASS_BLOCK,
        catalog::COBBLESTONE,
        catalog::OAK_PLANKS,
        catalog::SAND,
        catalog::SANDSTONE,
        catalog::GLASS,
        catalog::BRICKS,
    ]) {
        let _ = inventory.set(slot, Some(ItemStack::new(key, 64, 0)));
    }
    inventory
}

fn furnace_default_properties() -> BTreeMap<ContainerPropertyKey, i16> {
    BTreeMap::from([
        (ContainerPropertyKey::new(FURNACE_BURN_LEFT), 0),
        (ContainerPropertyKey::new(FURNACE_BURN_MAX), 0),
        (ContainerPropertyKey::new(FURNACE_COOK_PROGRESS), 0),
        (
            ContainerPropertyKey::new(FURNACE_COOK_TOTAL),
            FURNACE_COOK_TOTAL_DEFAULT,
        ),
    ])
}

fn default_block_entity_for_kind(kind: &BlockEntityKindId) -> Option<ContainerBlockEntityState> {
    match kind.as_str() {
        CHEST_BLOCK_ENTITY_KIND => Some(ContainerBlockEntityState {
            kind: kind.clone(),
            slots: vec![None; 27],
            properties: BTreeMap::new(),
        }),
        FURNACE_BLOCK_ENTITY_KIND => Some(ContainerBlockEntityState {
            kind: kind.clone(),
            slots: vec![None; 3],
            properties: furnace_default_properties(),
        }),
        _ => None,
    }
}

fn default_block_entity_for_block(block: &BlockState) -> Option<ContainerBlockEntityState> {
    match block.key.as_str() {
        catalog::CHEST => {
            default_block_entity_for_kind(&BlockEntityKindId::new(CHEST_BLOCK_ENTITY_KIND))
        }
        catalog::FURNACE => {
            default_block_entity_for_kind(&BlockEntityKindId::new(FURNACE_BLOCK_ENTITY_KIND))
        }
        _ => None,
    }
}

fn container_spec(kind: &ContainerKindId) -> Option<ContainerSpec> {
    Some(match kind.as_str() {
        PLAYER_KIND => ContainerSpec {
            local_slot_count: PLAYER_LOCAL_SLOT_COUNT,
            slot_roles: BTreeMap::from([
                (0, ContainerSlotRole::OutputOnly),
                (5, ContainerSlotRole::Unavailable),
                (6, ContainerSlotRole::Unavailable),
                (7, ContainerSlotRole::Unavailable),
                (8, ContainerSlotRole::Unavailable),
            ]),
        },
        CRAFTING_TABLE_KIND => ContainerSpec {
            local_slot_count: 10,
            slot_roles: BTreeMap::from([(0, ContainerSlotRole::OutputOnly)]),
        },
        CHEST_27_KIND => ContainerSpec {
            local_slot_count: 27,
            slot_roles: BTreeMap::new(),
        },
        FURNACE_KIND => ContainerSpec {
            local_slot_count: 3,
            slot_roles: BTreeMap::from([(2, ContainerSlotRole::OutputOnly)]),
        },
        _ => return None,
    })
}

impl mc_content_api::ContentBehavior for TestContentBehavior {
    fn generate_chunk(&self, _meta: &WorldMeta, chunk_pos: ChunkPos) -> ChunkColumn {
        let mut chunk = ChunkColumn::new(chunk_pos);
        for z in 0_u8..16 {
            for x in 0_u8..16 {
                chunk.set_block(x, 0, z, Some(BlockState::new(catalog::BEDROCK)));
                chunk.set_block(x, 1, z, Some(BlockState::new(catalog::STONE)));
                chunk.set_block(x, 2, z, Some(BlockState::new(catalog::DIRT)));
                chunk.set_block(x, 3, z, Some(BlockState::new(catalog::GRASS_BLOCK)));
            }
        }
        chunk
    }

    fn player_container_kind(&self) -> ContainerKindId {
        ContainerKindId::new(PLAYER_KIND)
    }

    fn container_spec(&self, kind: &ContainerKindId) -> Option<ContainerSpec> {
        container_spec(kind)
    }

    fn container_title(&self, kind: &ContainerKindId) -> String {
        match kind.as_str() {
            PLAYER_KIND => "Player".to_string(),
            CRAFTING_TABLE_KIND => "Crafting".to_string(),
            CHEST_27_KIND => "Chest".to_string(),
            FURNACE_KIND => "Furnace".to_string(),
            _ => kind.as_str().to_string(),
        }
    }

    fn container_kind_for_block(&self, block: &BlockState) -> Option<ContainerKindId> {
        match block.key.as_str() {
            catalog::CRAFTING_TABLE => Some(ContainerKindId::new(CRAFTING_TABLE_KIND)),
            catalog::CHEST => Some(ContainerKindId::new(CHEST_27_KIND)),
            catalog::FURNACE => Some(ContainerKindId::new(FURNACE_KIND)),
            _ => None,
        }
    }

    fn default_block_entity_for_block(
        &self,
        block: &BlockState,
    ) -> Option<ContainerBlockEntityState> {
        default_block_entity_for_block(block)
    }

    fn default_block_entity_for_kind(
        &self,
        kind: &BlockEntityKindId,
    ) -> Option<ContainerBlockEntityState> {
        default_block_entity_for_kind(kind)
    }

    fn block_entity_kind_for_container(&self, kind: &ContainerKindId) -> Option<BlockEntityKindId> {
        match kind.as_str() {
            CHEST_27_KIND => Some(BlockEntityKindId::new(CHEST_BLOCK_ENTITY_KIND)),
            FURNACE_KIND => Some(BlockEntityKindId::new(FURNACE_BLOCK_ENTITY_KIND)),
            _ => None,
        }
    }

    fn container_kind_for_block_entity(
        &self,
        entity: &ContainerBlockEntityState,
    ) -> Option<ContainerKindId> {
        match entity.kind.as_str() {
            CHEST_BLOCK_ENTITY_KIND => Some(ContainerKindId::new(CHEST_27_KIND)),
            FURNACE_BLOCK_ENTITY_KIND => Some(ContainerKindId::new(FURNACE_KIND)),
            _ => None,
        }
    }

    fn is_air_block(&self, block: &BlockState) -> bool {
        block.key.as_str() == "minecraft:air"
    }

    fn is_unbreakable_block(&self, block: &BlockState) -> bool {
        block.key.as_str() == catalog::BEDROCK
    }

    fn placeable_block_state_from_item_key(&self, key: &str) -> Option<BlockState> {
        mc_content_canonical::placeable_block_state_from_item_key(key)
    }

    fn is_supported_inventory_item(&self, key: &str) -> bool {
        mc_content_canonical::item_supported_for_inventory(key)
    }

    fn starter_inventory(&self) -> PlayerInventory {
        creative_starter_inventory()
    }

    fn tool_spec_for_item(&self, _item: Option<&ItemStack>) -> Option<MiningToolSpec> {
        None
    }

    fn survival_mining_duration_ms(
        &self,
        block: &BlockState,
        _tool: Option<MiningToolSpec>,
    ) -> Option<u64> {
        let hardness = match block.key.as_str() {
            catalog::STONE => 1.5,
            catalog::GRASS_BLOCK => 0.6,
            catalog::DIRT => 0.5,
            catalog::COBBLESTONE => 2.0,
            catalog::OAK_PLANKS => 2.0,
            catalog::SAND => 0.5,
            catalog::SANDSTONE => 0.8,
            catalog::GLASS => 0.3,
            catalog::BRICKS => 2.0,
            catalog::CRAFTING_TABLE => 2.5,
            catalog::CHEST => 2.5,
            catalog::FURNACE => 3.5,
            _ => return None,
        };
        let ticks = (hardness * 30.0_f32).ceil() as u64;
        Some(ticks.max(1) * 50)
    }

    fn survival_drop_for_block(&self, block: &BlockState) -> Option<ItemStack> {
        mc_content_canonical::canonical_content().survival_drop_for_block(block)
    }

    fn normalize_container(&self, state: &mut OpenContainerState) {
        match state.kind.as_str() {
            CRAFTING_TABLE_KIND => recompute_crafting_result(&mut state.local_slots, 3),
            FURNACE_KIND => normalize_furnace(state),
            _ => {}
        }
    }

    fn normalize_player_inventory(&self, inventory: &mut PlayerInventory) {
        recompute_crafting_result_for_player(inventory);
    }

    fn try_take_output(
        &self,
        kind: &ContainerKindId,
        local_slots: &mut Vec<Option<ItemStack>>,
        cursor: &mut Option<ItemStack>,
        button: InventoryClickButton,
    ) -> bool {
        match kind.as_str() {
            PLAYER_KIND => take_player_crafting_result(local_slots, cursor, button),
            CRAFTING_TABLE_KIND => take_container_crafting_result(local_slots, cursor, button),
            FURNACE_KIND => take_output_slot(local_slots, cursor, 2, button),
            _ => false,
        }
    }

    fn tick_container(&self, state: &mut OpenContainerState) {
        if state.kind.as_str() == FURNACE_KIND {
            tick_furnace(state);
        }
    }
}

fn take_output_slot(
    local_slots: &mut Vec<Option<ItemStack>>,
    cursor: &mut Option<ItemStack>,
    output_index: usize,
    button: InventoryClickButton,
) -> bool {
    let Some(output) = local_slots.get(output_index).and_then(Clone::clone) else {
        return false;
    };
    let take_count = match button {
        InventoryClickButton::Left => output.count,
        InventoryClickButton::Right => 1,
    };
    if take_count == 0 {
        return false;
    }
    match cursor.as_mut() {
        None => {
            let mut taken = output.clone();
            taken.count = take_count;
            *cursor = Some(taken);
        }
        Some(existing)
            if existing.key == output.key
                && u16::from(existing.count) + u16::from(take_count) <= 64 =>
        {
            existing.count = existing.count.saturating_add(take_count);
        }
        Some(_) => return false,
    }
    if let Some(slot) = local_slots.get_mut(output_index)
        && let Some(existing) = slot.as_mut()
    {
        existing.count = existing.count.saturating_sub(take_count);
        if existing.count == 0 {
            *slot = None;
        }
    }
    true
}

fn recompute_crafting_result_for_player(inventory: &mut PlayerInventory) {
    let mut local_slots = (0_u16..PLAYER_LOCAL_SLOT_COUNT)
        .map(|index| {
            inventory
                .get_slot(InventorySlot::WindowLocal(index))
                .cloned()
        })
        .collect::<Vec<_>>();
    recompute_crafting_result(&mut local_slots, 2);
    for (index, stack) in local_slots.into_iter().enumerate() {
        let _ = inventory.set_slot(
            InventorySlot::WindowLocal(u16::try_from(index).expect("player local slot fits")),
            stack,
        );
    }
}

fn take_player_crafting_result(
    local_slots: &mut Vec<Option<ItemStack>>,
    cursor: &mut Option<ItemStack>,
    button: InventoryClickButton,
) -> bool {
    take_crafting_result(local_slots, 1, 4, 2, cursor, button)
}

fn take_container_crafting_result(
    local_slots: &mut Vec<Option<ItemStack>>,
    cursor: &mut Option<ItemStack>,
    button: InventoryClickButton,
) -> bool {
    take_crafting_result(local_slots, 1, 9, 3, cursor, button)
}

fn take_crafting_result(
    local_slots: &mut Vec<Option<ItemStack>>,
    input_start: usize,
    input_count: usize,
    width: usize,
    cursor: &mut Option<ItemStack>,
    button: InventoryClickButton,
) -> bool {
    let inputs = local_slots
        .iter()
        .skip(input_start)
        .take(input_count)
        .cloned()
        .collect::<Vec<_>>();
    let Some((output, consume)) = current_crafting_recipe(&inputs, width) else {
        recompute_crafting_result(local_slots, width);
        return false;
    };
    let take_count = match button {
        InventoryClickButton::Left => output.count,
        InventoryClickButton::Right => 1,
    };
    match cursor.as_mut() {
        None => {
            let mut taken = output.clone();
            taken.count = take_count;
            *cursor = Some(taken);
        }
        Some(existing)
            if existing.key == output.key
                && u16::from(existing.count) + u16::from(take_count) <= 64 =>
        {
            existing.count = existing.count.saturating_add(take_count);
        }
        Some(_) => return false,
    }
    for index in consume {
        if let Some(Some(stack)) = local_slots.get_mut(input_start + index) {
            stack.count = stack.count.saturating_sub(1);
            if stack.count == 0 {
                local_slots[input_start + index] = None;
            }
        }
    }
    recompute_crafting_result(local_slots, width);
    true
}

fn recompute_crafting_result(local_slots: &mut [Option<ItemStack>], width: usize) {
    let inputs = if width == 2 {
        local_slots
            .iter()
            .skip(1)
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        local_slots
            .iter()
            .skip(1)
            .take(9)
            .cloned()
            .collect::<Vec<_>>()
    };
    local_slots[0] = current_crafting_recipe(&inputs, width).map(|(output, _)| output);
}

fn current_crafting_recipe(
    inputs: &[Option<ItemStack>],
    width: usize,
) -> Option<(ItemStack, Vec<usize>)> {
    let occupied = inputs
        .iter()
        .enumerate()
        .filter_map(|(index, stack)| stack.as_ref().map(|stack| (index, stack)))
        .collect::<Vec<_>>();
    if occupied.is_empty() {
        return None;
    }
    let min_x = occupied.iter().map(|(index, _)| index % width).min()?;
    let max_x = occupied.iter().map(|(index, _)| index % width).max()?;
    let min_y = occupied.iter().map(|(index, _)| index / width).min()?;
    let max_y = occupied.iter().map(|(index, _)| index / width).max()?;
    let normalized_width = max_x - min_x + 1;
    let normalized_height = max_y - min_y + 1;
    let mut cells = vec![None; normalized_width * normalized_height];
    let mut original_indices = vec![None; normalized_width * normalized_height];
    for (index, stack) in occupied {
        let x = index % width - min_x;
        let y = index / width - min_y;
        let normalized = y * normalized_width + x;
        cells[normalized] = Some(stack.key.as_str());
        original_indices[normalized] = Some(index);
    }
    let defs = [
        (
            vec![Some(catalog::OAK_LOG)],
            1,
            1,
            catalog::OAK_PLANKS,
            4_u8,
        ),
        (
            vec![Some(catalog::OAK_PLANKS), Some(catalog::OAK_PLANKS)],
            1,
            2,
            catalog::STICK,
            4_u8,
        ),
        (
            vec![
                Some(catalog::SAND),
                Some(catalog::SAND),
                Some(catalog::SAND),
                Some(catalog::SAND),
            ],
            2,
            2,
            catalog::SANDSTONE,
            1_u8,
        ),
        (
            vec![
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
            ],
            2,
            2,
            catalog::CRAFTING_TABLE,
            1_u8,
        ),
        (
            vec![
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                None,
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
            ],
            3,
            3,
            catalog::CHEST,
            1_u8,
        ),
        (
            vec![
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
                None,
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
            ],
            3,
            3,
            catalog::FURNACE,
            1_u8,
        ),
    ];
    for (pattern, pattern_width, pattern_height, output_key, output_count) in defs {
        if pattern_width != normalized_width || pattern_height != normalized_height {
            continue;
        }
        if pattern
            .iter()
            .zip(&cells)
            .all(|(expected, actual)| *expected == actual.as_ref().copied())
        {
            let consume = original_indices.into_iter().flatten().collect::<Vec<_>>();
            return Some((ItemStack::new(output_key, output_count, 0), consume));
        }
    }
    None
}

fn normalize_furnace(state: &mut OpenContainerState) {
    state.properties.insert(
        ContainerPropertyKey::new(FURNACE_COOK_TOTAL),
        FURNACE_COOK_TOTAL_DEFAULT,
    );
    let available_output = furnace_available_output(&state.local_slots);
    if state
        .properties
        .get(&ContainerPropertyKey::new(FURNACE_BURN_LEFT))
        .copied()
        .unwrap_or_default()
        == 0
    {
        state
            .properties
            .insert(ContainerPropertyKey::new(FURNACE_BURN_MAX), 0);
    }
    if available_output.is_none() {
        state
            .properties
            .insert(ContainerPropertyKey::new(FURNACE_COOK_PROGRESS), 0);
    }
}

fn furnace_available_output(local_slots: &[Option<ItemStack>]) -> Option<ItemStack> {
    let input = local_slots.first().and_then(Option::as_ref)?;
    let output = match input.key.as_str() {
        catalog::SAND if input.count > 0 => ItemStack::new(catalog::GLASS, 1, 0),
        catalog::COBBLESTONE if input.count > 0 => ItemStack::new(catalog::STONE, 1, 0),
        _ => return None,
    };
    match local_slots.get(2).and_then(Option::as_ref) {
        None => Some(output),
        Some(existing) if existing.key == output.key && existing.count < 64 => Some(output),
        _ => None,
    }
}

fn furnace_fuel_burn_time(fuel: Option<&ItemStack>) -> Option<i16> {
    let fuel = fuel?;
    if fuel.count == 0 {
        return None;
    }
    match fuel.key.as_str() {
        catalog::STICK => Some(100),
        catalog::OAK_PLANKS | catalog::OAK_LOG => Some(300),
        _ => None,
    }
}

fn tick_furnace(state: &mut OpenContainerState) {
    normalize_furnace(state);
    let burn_left_key = ContainerPropertyKey::new(FURNACE_BURN_LEFT);
    let burn_max_key = ContainerPropertyKey::new(FURNACE_BURN_MAX);
    let cook_progress_key = ContainerPropertyKey::new(FURNACE_COOK_PROGRESS);
    let cook_total = state
        .properties
        .get(&ContainerPropertyKey::new(FURNACE_COOK_TOTAL))
        .copied()
        .unwrap_or(FURNACE_COOK_TOTAL_DEFAULT);
    let burn_left = state.properties.get(&burn_left_key).copied().unwrap_or(0);
    if burn_left > 0 {
        state
            .properties
            .insert(burn_left_key.clone(), burn_left.saturating_sub(1));
    }
    let current_burn_left = state.properties.get(&burn_left_key).copied().unwrap_or(0);
    if current_burn_left == 0
        && furnace_available_output(&state.local_slots).is_some()
        && let Some(burn_time) =
            furnace_fuel_burn_time(state.local_slots.get(1).and_then(Option::as_ref))
    {
        state.properties.insert(burn_left_key.clone(), burn_time);
        state.properties.insert(burn_max_key.clone(), burn_time);
        if let Some(Some(fuel)) = state.local_slots.get_mut(1) {
            fuel.count = fuel.count.saturating_sub(1);
            if fuel.count == 0 {
                state.local_slots[1] = None;
            }
        }
    }
    let active_burn = state.properties.get(&burn_left_key).copied().unwrap_or(0);
    if active_burn > 0 {
        if let Some(output) = furnace_available_output(&state.local_slots) {
            let progress = state
                .properties
                .get(&cook_progress_key)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            if progress >= cook_total {
                if let Some(Some(input)) = state.local_slots.get_mut(0) {
                    input.count = input.count.saturating_sub(1);
                    if input.count == 0 {
                        state.local_slots[0] = None;
                    }
                }
                match state.local_slots.get_mut(2) {
                    Some(Some(existing)) if existing.key == output.key => {
                        existing.count = existing.count.saturating_add(output.count);
                    }
                    Some(slot) => *slot = Some(output),
                    None => {}
                }
                state.properties.insert(cook_progress_key, 0);
            } else {
                state.properties.insert(cook_progress_key, progress);
            }
        } else {
            state.properties.insert(cook_progress_key, 0);
        }
    } else {
        state.properties.insert(cook_progress_key, 0);
        state.properties.insert(burn_max_key, 0);
    }
}

#[derive(Clone, Debug)]
pub(super) struct OnlinePlayerState {
    pub(super) snapshot: PlayerSnapshot,
    pub(super) cursor: Option<ItemStack>,
    pub(super) active_container: Option<crate::core::OpenInventoryWindow>,
    pub(super) active_mining: Option<crate::core::ActiveMiningState>,
}

pub(super) fn online_player(core: &ServerCore, player_id: PlayerId) -> OnlinePlayerState {
    let session = core
        .player_session(player_id)
        .expect("player should still be online")
        .clone();
    let snapshot = core
        .compose_player_snapshot(player_id)
        .expect("player snapshot should compose");
    OnlinePlayerState {
        snapshot,
        cursor: session.cursor,
        active_container: session.active_container,
        active_mining: core.player_active_mining(player_id).cloned(),
    }
}

pub(super) fn active_container_mut(
    core: &mut ServerCore,
    player_id: PlayerId,
) -> &mut crate::core::OpenInventoryWindow {
    core.player_session_mut(player_id)
        .and_then(|session| session.active_container.as_mut())
        .expect("player should have an active container")
}

pub(super) fn dropped_item_snapshot(
    core: &ServerCore,
    entity_id: EntityId,
) -> crate::DroppedItemSnapshot {
    core.entities
        .dropped_items
        .get(&entity_id)
        .expect("dropped item should still exist")
        .snapshot
        .clone()
}

pub(super) fn stack_summary(stack: &ItemStack) -> (&str, u8) {
    (stack.key.as_str(), stack.count)
}

#[track_caller]
pub(super) fn assert_connection_event<F>(
    events: &[TargetedEvent],
    connection_id: ConnectionId,
    predicate: F,
) where
    F: Fn(&CoreEvent) -> bool,
{
    assert!(events.iter().any(|event| {
        matches!(event.target, EventTarget::Connection(id) if id == connection_id)
            && predicate(&event.event)
    }));
}

#[track_caller]
pub(super) fn assert_player_event<F>(events: &[TargetedEvent], player_id: PlayerId, predicate: F)
where
    F: Fn(&CoreEvent) -> bool,
{
    assert!(events.iter().any(|event| {
        matches!(event.target, EventTarget::Player(id) if id == player_id)
            && predicate(&event.event)
    }));
}

#[track_caller]
pub(super) fn assert_everyone_except_event<F>(
    events: &[TargetedEvent],
    player_id: PlayerId,
    predicate: F,
) where
    F: Fn(&CoreEvent) -> bool,
{
    assert!(events.iter().any(|event| {
        matches!(event.target, EventTarget::EveryoneExcept(id) if id == player_id)
            && predicate(&event.event)
    }));
}

pub(super) fn count_player_events<F>(
    events: &[TargetedEvent],
    player_id: PlayerId,
    predicate: F,
) -> usize
where
    F: Fn(&CoreEvent) -> bool,
{
    events
        .iter()
        .filter(|event| {
            matches!(event.target, EventTarget::Player(id) if id == player_id)
                && predicate(&event.event)
        })
        .count()
}

#[track_caller]
pub(super) fn assert_transaction_processed(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    action_number: i16,
    accepted: bool,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::InventoryTransactionProcessed {
                transaction,
                accepted: event_accepted,
            } if *event_accepted == accepted
                && *transaction == InventoryTransactionContext {
                    window_id,
                    action_number,
                }
        )
    });
}

#[track_caller]
pub(super) fn assert_inventory_slot_changed_in_window(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    slot: InventorySlot,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::InventorySlotChanged {
                window_id: event_window_id,
                slot: event_slot,
                ..
            } if *event_window_id == window_id && *event_slot == slot
        )
    });
}

#[track_caller]
pub(super) fn assert_inventory_slot_changed_to(
    events: &[TargetedEvent],
    player_id: PlayerId,
    slot: InventorySlot,
    expected: Option<(&str, u8)>,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::InventorySlotChanged {
            slot: event_slot,
            stack,
            ..
        } if *event_slot == slot => stack.as_ref().map(stack_summary) == expected,
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_inventory_slot_changed_in_window_to(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    slot: InventorySlot,
    expected: Option<(&str, u8)>,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::InventorySlotChanged {
            window_id: event_window_id,
            slot: event_slot,
            stack,
            ..
        } if *event_window_id == window_id && *event_slot == slot => {
            stack.as_ref().map(stack_summary) == expected
        }
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_cursor_changed_to(
    events: &[TargetedEvent],
    player_id: PlayerId,
    key: &str,
    count: u8,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::CursorChanged { stack } => {
            stack.as_ref().map(stack_summary) == Some((key, count))
        }
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_player_window_contents(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    container: InventoryContainer,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::InventoryContents {
                window_id: event_window_id,
                container: event_container,
                ..
            } if *event_window_id == window_id
                && *event_container == container_kind(container)
        )
    });
}

#[track_caller]
pub(super) fn assert_container_opened(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    container: InventoryContainer,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::ContainerOpened {
                window_id: event_window_id,
                container: event_container,
                ..
            } if *event_window_id == window_id
                && *event_container == container_kind(container)
        )
    });
}

#[track_caller]
pub(super) fn assert_container_closed(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::ContainerClosed {
                window_id: event_window_id,
            } if *event_window_id == window_id
        )
    });
}

#[track_caller]
pub(super) fn assert_container_property_changed(
    events: &[TargetedEvent],
    player_id: PlayerId,
    window_id: u8,
    property_id: u8,
    value: i16,
) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::ContainerPropertyChanged {
                window_id: event_window_id,
                property: event_property_id,
                value: event_value,
            } if *event_window_id == window_id
                && *event_property_id == furnace_property_key(property_id)
                && *event_value == value
        )
    });
}

#[track_caller]
pub(super) fn assert_connection_inventory_contents(
    events: &[TargetedEvent],
    connection_id: ConnectionId,
) {
    assert_connection_event(events, connection_id, |event| {
        matches!(
            event,
            CoreEvent::InventoryContents {
                container,
                ..
            } if *container == container_kind(InventoryContainer::Player)
        )
    });
}

#[track_caller]
pub(super) fn assert_connection_dropped_item_spawned(
    events: &[TargetedEvent],
    connection_id: ConnectionId,
    expected: Option<(&str, u8)>,
) {
    assert_connection_event(events, connection_id, |event| match event {
        CoreEvent::DroppedItemSpawned { item, .. } => Some(stack_summary(&item.item)) == expected,
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_player_dropped_item_spawned(
    events: &[TargetedEvent],
    player_id: PlayerId,
    expected: Option<(&str, u8)>,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::DroppedItemSpawned { item, .. } => Some(stack_summary(&item.item)) == expected,
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_player_dropped_item_spawned_at(
    events: &[TargetedEvent],
    player_id: PlayerId,
    key: &str,
    count: u8,
    position: Vec3,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::DroppedItemSpawned { item, .. } => {
            stack_summary(&item.item) == (key, count) && item.position == position
        }
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_player_entity_despawned(
    events: &[TargetedEvent],
    player_id: PlayerId,
    entity_id: EntityId,
) {
    assert_player_event(events, player_id, |event| match event {
        CoreEvent::EntityDespawned { entity_ids } => entity_ids.contains(&entity_id),
        _ => false,
    });
}

#[track_caller]
pub(super) fn assert_player_inventory_contents(events: &[TargetedEvent], player_id: PlayerId) {
    assert_player_event(events, player_id, |event| {
        matches!(
            event,
            CoreEvent::InventoryContents {
                container,
                ..
            } if *container == container_kind(InventoryContainer::Player)
        )
    });
}

#[track_caller]
pub(super) fn assert_connection_selected_hotbar_slot(
    events: &[TargetedEvent],
    connection_id: ConnectionId,
    slot: u8,
) {
    assert_connection_event(
        events,
        connection_id,
        |event| matches!(event, CoreEvent::SelectedHotbarSlotChanged { slot: event_slot } if *event_slot == slot),
    );
}

#[track_caller]
pub(super) fn assert_player_selected_hotbar_slot(
    events: &[TargetedEvent],
    player_id: PlayerId,
    slot: u8,
) {
    assert_player_event(
        events,
        player_id,
        |event| matches!(event, CoreEvent::SelectedHotbarSlotChanged { slot: event_slot } if *event_slot == slot),
    );
}

#[track_caller]
pub(super) fn assert_crafting_inputs_empty(inventory: &PlayerInventory) {
    for index in 0_u8..4 {
        assert!(
            inventory.crafting_input(index).is_none(),
            "craft input {index} should be consumed"
        );
    }
}
