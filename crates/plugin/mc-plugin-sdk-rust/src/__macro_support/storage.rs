use crate::storage::RustStoragePlugin;
use mc_plugin_api::codec::storage::{StorageRequest, StorageResponse};
use mc_proto_common::StorageError;
use std::path::Path;

pub fn handle_storage_request<P: RustStoragePlugin>(
    plugin: &P,
    request: StorageRequest,
) -> Result<StorageResponse, String> {
    match request {
        StorageRequest::Describe => Ok(StorageResponse::Descriptor(plugin.descriptor())),
        StorageRequest::CapabilitySet => Ok(StorageResponse::CapabilitySet(
            crate::capabilities::storage_announcement(&plugin.capability_set()),
        )),
        StorageRequest::LoadSnapshot { world_dir } => plugin
            .load_snapshot(Path::new(&world_dir))
            .map(StorageResponse::Snapshot)
            .map_err(storage_error_to_string),
        StorageRequest::SaveSnapshot {
            world_dir,
            snapshot,
        } => plugin
            .save_snapshot(Path::new(&world_dir), &snapshot)
            .map(|()| StorageResponse::Empty)
            .map_err(storage_error_to_string),
        StorageRequest::ExportRuntimeState { world_dir } => plugin
            .export_runtime_state(Path::new(&world_dir))
            .map(StorageResponse::Snapshot)
            .map_err(storage_error_to_string),
        StorageRequest::ImportRuntimeState {
            world_dir,
            snapshot,
        } => plugin
            .import_runtime_state(Path::new(&world_dir), &snapshot)
            .map(|()| StorageResponse::Empty)
            .map_err(storage_error_to_string),
    }
}

fn storage_error_to_string(error: StorageError) -> String {
    error.to_string()
}
