use serde::Serialize;
use std::fs;
use std::path::PathBuf;

const DEFAULT_FILE_READ_MAX_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_FILE_WRITE_MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileReadResponse {
    pub path: String,
    pub text: String,
    pub bytes: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileWriteResponse {
    pub path: String,
    pub bytes_written: usize,
}

fn parse_absolute_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Path is required.".to_string());
    }

    let parsed = PathBuf::from(trimmed);
    if !parsed.is_absolute() {
        return Err("Use an absolute path.".to_string());
    }

    Ok(parsed)
}

#[tauri::command]
pub fn file_exists(path: String) -> Result<bool, String> {
    let parsed = parse_absolute_path(&path)?;
    Ok(parsed.exists())
}

#[tauri::command]
pub fn file_read(path: String, max_bytes: Option<usize>) -> Result<FileReadResponse, String> {
    let parsed = parse_absolute_path(&path)?;
    let cap = max_bytes
        .unwrap_or(DEFAULT_FILE_READ_MAX_BYTES)
        .min(DEFAULT_FILE_READ_MAX_BYTES);

    let metadata = fs::metadata(&parsed).map_err(|_| "Unable to read file metadata.".to_string())?;
    if !metadata.is_file() {
        return Err("Path is not a file.".to_string());
    }
    if metadata.len() as usize > cap {
        return Err("File is too large to read safely.".to_string());
    }

    let raw = fs::read(&parsed).map_err(|_| "Unable to read file.".to_string())?;
    if raw.len() > cap {
        return Err("File is too large to read safely.".to_string());
    }
    let bytes_len = raw.len();
    let text = String::from_utf8(raw).map_err(|_| "Only UTF-8 text files are supported.".to_string())?;

    Ok(FileReadResponse {
        path: parsed.to_string_lossy().to_string(),
        text,
        bytes: bytes_len,
    })
}

#[tauri::command]
pub fn file_write(
    path: String,
    text: String,
    create_if_missing: Option<bool>,
    max_bytes: Option<usize>,
) -> Result<FileWriteResponse, String> {
    let parsed = parse_absolute_path(&path)?;
    let cap = max_bytes
        .unwrap_or(DEFAULT_FILE_WRITE_MAX_BYTES)
        .min(DEFAULT_FILE_WRITE_MAX_BYTES);

    let bytes_len = text.len();
    if bytes_len > cap {
        return Err("Text is too large to write safely.".to_string());
    }

    let create = create_if_missing.unwrap_or(false);
    if !create && !parsed.exists() {
        return Err("File does not exist.".to_string());
    }

    let parent = parsed
        .parent()
        .ok_or_else(|| "Parent folder is invalid.".to_string())?;
    if !parent.exists() {
        return Err("Parent folder does not exist.".to_string());
    }

    fs::write(&parsed, text).map_err(|_| "Unable to write file.".to_string())?;

    Ok(FileWriteResponse {
        path: parsed.to_string_lossy().to_string(),
        bytes_written: bytes_len,
    })
}
