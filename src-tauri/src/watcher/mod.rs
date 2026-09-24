pub mod fs_watcher;
pub use fs_watcher::spawn_vault_watcher;
#[allow(unused_imports)]
pub use fs_watcher::VaultEvent;
#[allow(unused_imports)]
pub use fs_watcher::VaultLock;
pub use fs_watcher::WatcherHandle;
