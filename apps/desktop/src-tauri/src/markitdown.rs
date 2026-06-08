use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdDetectResponse {
    pub python_found: bool,
    pub markitdown_found: bool,
    pub venv_path: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdInstallResponse {
    pub logs: String,
    pub venv_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdConvertResponse {
    pub markdown: String,
    pub input_bytes: u64,
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|_| "Unable to resolve app data directory".to_string())
}

fn venv_path_from_data_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("markitdown-venv")
}

fn venv_python_path(venv_path: &Path) -> PathBuf {
    venv_path.join("bin").join("python")
}

fn python3_exists() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn has_markitdown_with_python(python_executable: &Path) -> bool {
    Command::new(python_executable)
        .args(["-c", "import markitdown"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn has_markitdown_with_python3() -> bool {
    Command::new("python3")
        .args(["-c", "import markitdown"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn command_output_line(output: &[u8]) -> String {
    String::from_utf8_lossy(output).trim().to_string()
}

fn run_step(command: &mut Command, step_name: &str, logs: &mut Vec<String>) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|_| format!("{step_name} failed to start"))?;

    let stdout_line = command_output_line(&output.stdout);
    if !stdout_line.is_empty() {
        logs.push(format!("[{step_name}] {stdout_line}"));
    }

    let stderr_line = command_output_line(&output.stderr);
    if !stderr_line.is_empty() {
        logs.push(format!("[{step_name}] {stderr_line}"));
    }

    if output.status.success() {
        return Ok(());
    }

    let failure_message = if !stderr_line.is_empty() {
        stderr_line
    } else if !stdout_line.is_empty() {
        stdout_line
    } else {
        "command returned non-zero exit status".to_string()
    };

    Err(format!("{step_name} failed: {failure_message}"))
}

fn resolve_markitdown_python(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app_data_dir(app)?;
    let venv_path = venv_path_from_data_dir(&data_dir);
    let venv_python = venv_python_path(&venv_path);

    if venv_python.exists() && has_markitdown_with_python(&venv_python) {
        return Ok(venv_python);
    }

    if has_markitdown_with_python3() {
        return Ok(PathBuf::from("python3"));
    }

    Err("MarkItDown is not installed yet. Click Install first.".to_string())
}

#[tauri::command]
pub fn md_detect(app: AppHandle) -> Result<MdDetectResponse, String> {
    let data_dir = app_data_dir(&app)?;
    let venv_path = venv_path_from_data_dir(&data_dir);
    let venv_python = venv_python_path(&venv_path);

    let python_found = python3_exists() || venv_python.exists();
    let markitdown_found = if venv_python.exists() {
        has_markitdown_with_python(&venv_python)
    } else {
        has_markitdown_with_python3()
    };

    Ok(MdDetectResponse {
        python_found,
        markitdown_found,
        venv_path: if venv_path.exists() {
            Some(venv_path.to_string_lossy().to_string())
        } else {
            None
        },
    })
}

#[tauri::command]
pub fn md_install(app: AppHandle) -> Result<MdInstallResponse, String> {
    if !python3_exists() {
        return Err("python3 was not found. Install Python 3 and try again.".to_string());
    }

    let data_dir = app_data_dir(&app)?;
    fs::create_dir_all(&data_dir)
        .map_err(|_| "Unable to prepare app data directory for MarkItDown".to_string())?;

    let venv_path = venv_path_from_data_dir(&data_dir);
    let venv_python = venv_python_path(&venv_path);
    let mut logs = Vec::new();

    if !venv_python.exists() {
        run_step(
            Command::new("python3").arg("-m").arg("venv").arg(&venv_path),
            "create_venv",
            &mut logs,
        )?;
    } else {
        logs.push("[create_venv] Existing virtual environment detected.".to_string());
    }

    run_step(
        Command::new(&venv_python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("pip"),
        "upgrade_pip",
        &mut logs,
    )?;

    run_step(
        Command::new(&venv_python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("markitdown"),
        "install_markitdown",
        &mut logs,
    )?;

    if !has_markitdown_with_python(&venv_python) {
        return Err("MarkItDown install completed but import check failed.".to_string());
    }

    Ok(MdInstallResponse {
        logs: logs.join("\n"),
        venv_path: venv_path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub fn md_convert(app: AppHandle, file_path: String) -> Result<MdConvertResponse, String> {
    let file_path = PathBuf::from(&file_path);

    if !file_path.exists() {
        return Err("Selected file does not exist.".to_string());
    }

    if !file_path.is_file() {
        return Err("Selected path is not a file.".to_string());
    }

    let input_bytes = fs::metadata(&file_path)
        .map(|metadata| metadata.len())
        .map_err(|_| "Unable to read file metadata".to_string())?;

    let python_executable = resolve_markitdown_python(&app)?;
    let output = Command::new(python_executable)
        .arg("-m")
        .arg("markitdown")
        .arg(&file_path)
        .output()
        .map_err(|_| "Failed to run MarkItDown conversion".to_string())?;

    if !output.status.success() {
        let error_line = command_output_line(&output.stderr);
        if error_line.is_empty() {
            return Err("MarkItDown conversion failed.".to_string());
        }
        return Err(format!("MarkItDown conversion failed: {error_line}"));
    }

    Ok(MdConvertResponse {
        markdown: String::from_utf8_lossy(&output.stdout).to_string(),
        input_bytes,
    })
}
