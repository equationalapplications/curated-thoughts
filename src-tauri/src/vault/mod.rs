pub mod config;
pub mod layout;
pub mod safe_path;
pub use config::VaultConfig;
pub use safe_path::{
    safe_vault_path, safe_write_bytes, PathMode, SafePathError, IMMUTABLE_DIR, WIKI_DIR,
    READABLE_SUBDIRS, WRITABLE_SUBDIRS, PROPOSED_SUBDIRS, NOTE_WRITABLE_SUBDIRS,
    AGENTS_DEPOSIT_DIR,
};
