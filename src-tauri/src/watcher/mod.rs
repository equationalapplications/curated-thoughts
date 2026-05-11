pub mod fs_watcher;
pub use fs_watcher::spawn_vault_watcher;
pub use fs_watcher::WatcherHandle;
#[allow(unused_imports)]
pub use fs_watcher::VaultEvent;
