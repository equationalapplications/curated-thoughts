//! Tauri commands for OKF bundle export/import (UX spec §2, phase 6).

use std::path::PathBuf;

use serde::Serialize;
use tauri::State;

use crate::db::bundle_apply::{apply_import, preview_import, ImportMode, ImportPreview, ImportResult};
use crate::db::bundle_io::load_export_entities;
use crate::okf::bundle_read::parse_bundle;
use crate::okf::bundle_write::write_bundle;
use crate::okf::zip_io::{read_bundle_source, write_bundle_zip};
use crate::DbState;

#[derive(Debug, Serialize)]
pub struct ExportSummary {
    pub path: String,
    pub entities: usize,
    pub files: usize,
}

#[tauri::command]
pub fn okf_export_bundle_cmd(
    dest_path: String,
    entity_ids: Option<Vec<String>>,
    db_state: State<DbState>,
) -> Result<ExportSummary, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    let entities =
        load_export_entities(&guard.0, entity_ids.as_deref()).map_err(|e| e.to_string())?;
    if entities.is_empty() {
        return Err("Nothing to export: no entities in the brain.".into());
    }
    let count = entities.len();
    let files = write_bundle(&entities);
    write_bundle_zip(&PathBuf::from(&dest_path), &files).map_err(|e| e.to_string())?;
    Ok(ExportSummary {
        path: dest_path,
        entities: count,
        files: files.len(),
    })
}

fn parse_mode(mode: &str) -> Result<ImportMode, String> {
    match mode {
        "merge" => Ok(ImportMode::Merge),
        "replace" => Ok(ImportMode::Replace),
        "clone" => Ok(ImportMode::Clone),
        other => Err(format!("unknown import mode: {other}")),
    }
}

#[tauri::command]
pub fn okf_import_preview_cmd(
    src_path: String,
    mode: String,
    db_state: State<DbState>,
) -> Result<ImportPreview, String> {
    let mode = parse_mode(&mode)?;
    let files = read_bundle_source(&PathBuf::from(&src_path)).map_err(|e| e.to_string())?;
    let bundle = parse_bundle(&files).map_err(|e| e.to_string())?;
    if bundle.entities.is_empty() {
        return Err("Not an OKF bundle: no entities found.".into());
    }
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    preview_import(&guard.0, &bundle, mode).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn okf_import_apply_cmd(
    src_path: String,
    mode: String,
    db_state: State<DbState>,
) -> Result<ImportResult, String> {
    let mode = parse_mode(&mode)?;
    let files = read_bundle_source(&PathBuf::from(&src_path)).map_err(|e| e.to_string())?;
    let bundle = parse_bundle(&files).map_err(|e| e.to_string())?;
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    apply_import(&mut guard.0, &bundle, mode).map_err(|e| e.to_string())
}
