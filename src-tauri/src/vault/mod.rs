pub mod config;
pub mod layout;
pub mod safe_path;
pub use config::VaultConfig;
pub use safe_path::{
    safe_vault_path, safe_write_bytes, PathMode, SafePathError, AGENTS_DEPOSIT_DIR, IMMUTABLE_DIR,
    NOTE_WRITABLE_SUBDIRS, PROPOSED_SUBDIRS, READABLE_SUBDIRS, WIKI_DIR, WRITABLE_SUBDIRS,
};
