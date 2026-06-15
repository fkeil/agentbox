pub mod agents;
pub mod auth;
pub mod config;
pub mod container;
pub mod engine;
pub mod manifest;
pub mod manifest_store;
pub mod notify;
pub mod profile;
pub mod provider;
pub mod sync;

pub use config::BackendChoice;
pub use container::{BoxInfo, CacheImage, ContainerStats, ContainerStatus};
pub use engine::{
    apply_snapshot_diff, attach_box, down_box, dry_run_box, find_manifests_dir_pub,
    get_container_stats, kill_box, list_boxes, list_cache_images, remove_box, remove_cache_image,
    run_box, run_box_config, stop_box, EngineError,
};
pub use manifest_store::{
    add_manifest, find_manifest_with_user_store, list_user_manifests, remove_manifest,
    ManifestStoreError, UserManifestEntry,
};
pub use profile::{
    export_profile_yaml, import_profile_yaml, list_profiles, load_profile, profiles_dir,
    remove_profile, save_profile, Profile, ProfileError,
};
pub use sync::{diff_path_for, load_diff, DiffKind, FileDiff};
