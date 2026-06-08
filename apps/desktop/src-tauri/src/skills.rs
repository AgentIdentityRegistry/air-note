use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const REGISTRY_PATH: &str = "skills/verified/registry.json";
const INSTALLED_SKILLS_FILE: &str = "installed_skills.json";

#[derive(Debug, Deserialize)]
struct VerifiedRegistry {
    channel: String,
    skills: Vec<VerifiedRegistryEntry>,
}

#[derive(Debug, Deserialize, Clone)]
struct VerifiedRegistryEntry {
    id: String,
    path: String,
    #[serde(default)]
    featured: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillBundlePreview {
    pub id: String,
    pub path: String,
    pub featured: bool,
    pub manifest_json: Option<String>,
    pub skill_md: Option<String>,
    pub prompt_md: Option<String>,
    pub load_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedSkillsListResponse {
    pub channel: String,
    pub skills: Vec<SkillBundlePreview>,
    pub installed: Vec<InstalledSkillRecord>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkillRecord {
    pub id: String,
    pub version: String,
    pub installed_at: String,
    pub channel: String,
    pub install_dir: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct InstalledSkillState {
    #[serde(default)]
    skills: Vec<InstalledSkillRecord>,
}

fn find_repo_root() -> Result<PathBuf, String> {
    let mut candidates = Vec::new();

    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir);
    }

    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    for candidate in candidates {
        for ancestor in candidate.ancestors() {
            let registry_path = ancestor.join(REGISTRY_PATH);
            if registry_path.exists() {
                return Ok(ancestor.to_path_buf());
            }
        }
    }

    Err("Unable to locate local verified skills registry in this environment.".to_string())
}

fn registry_file_path(repo_root: &Path) -> PathBuf {
    repo_root.join(REGISTRY_PATH)
}

fn read_registry(repo_root: &Path) -> Result<VerifiedRegistry, String> {
    let registry_path = registry_file_path(repo_root);
    let raw = fs::read_to_string(&registry_path)
        .map_err(|_| "Unable to read verified skills registry file.".to_string())?;

    serde_json::from_str::<VerifiedRegistry>(&raw)
        .map_err(|_| "Verified skills registry JSON is invalid.".to_string())
}

fn resolve_bundle_dir(repo_root: &Path, entry_path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(entry_path);
    if path.is_absolute() {
        return Err("Registry entry path must be relative.".to_string());
    }

    let joined = repo_root.join(path);
    if !joined.exists() {
        return Err("Skill bundle directory does not exist.".to_string());
    }

    if !joined.is_dir() {
        return Err("Skill bundle path is not a directory.".to_string());
    }

    let canonical_repo = repo_root
        .canonicalize()
        .map_err(|_| "Unable to resolve repository root path.".to_string())?;
    let canonical_bundle = joined
        .canonicalize()
        .map_err(|_| "Unable to resolve skill bundle path.".to_string())?;

    if !canonical_bundle.starts_with(&canonical_repo) {
        return Err("Registry entry path escapes repository root.".to_string());
    }

    Ok(canonical_bundle)
}

fn read_optional_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|_| "Unable to access app data directory".to_string())
}

fn installed_state_path(app: &AppHandle) -> Result<PathBuf, String> {
    let app_data = app_data_dir(app)?;
    fs::create_dir_all(&app_data)
        .map_err(|_| "Unable to initialize app data directory".to_string())?;
    Ok(app_data.join(INSTALLED_SKILLS_FILE))
}

fn load_installed_state(app: &AppHandle) -> Result<InstalledSkillState, String> {
    let file_path = installed_state_path(app)?;
    match fs::read_to_string(file_path) {
        Ok(contents) => serde_json::from_str::<InstalledSkillState>(&contents)
            .map_err(|_| "Installed skills state file is invalid JSON.".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(InstalledSkillState::default()),
        Err(_) => Err("Unable to read installed skills state file.".to_string()),
    }
}

fn save_installed_state(app: &AppHandle, state: &InstalledSkillState) -> Result<(), String> {
    let file_path = installed_state_path(app)?;
    let contents = serde_json::to_string_pretty(state)
        .map_err(|_| "Unable to serialize installed skills state.".to_string())?;
    fs::write(file_path, contents).map_err(|_| "Unable to write installed skills state file.".to_string())
}

fn now_unix_seconds_string() -> Result<String, String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Unable to compute current time.".to_string())?
        .as_secs();
    Ok(seconds.to_string())
}

fn copy_if_exists(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }

    fs::copy(source, destination).map_err(|_| "Unable to copy optional skill asset file.".to_string())?;
    Ok(())
}

#[tauri::command]
pub fn skills_list_verified(app: AppHandle) -> Result<VerifiedSkillsListResponse, String> {
    let repo_root = find_repo_root()?;
    let registry = read_registry(&repo_root)?;
    let installed_state = load_installed_state(&app)?;

    let mut previews = Vec::new();

    for entry in &registry.skills {
        let mut preview = SkillBundlePreview {
            id: entry.id.clone(),
            path: entry.path.clone(),
            featured: entry.featured,
            manifest_json: None,
            skill_md: None,
            prompt_md: None,
            load_error: None,
        };

        match resolve_bundle_dir(&repo_root, &entry.path) {
            Ok(bundle_dir) => {
                let manifest_path = bundle_dir.join("manifest.json");
                let skill_path = bundle_dir.join("SKILL.md");
                let prompt_path = bundle_dir.join("PROMPT.md");

                preview.manifest_json = read_optional_file(&manifest_path);
                preview.skill_md = read_optional_file(&skill_path);
                preview.prompt_md = read_optional_file(&prompt_path);

                if preview.manifest_json.is_none()
                    || preview.skill_md.is_none()
                    || preview.prompt_md.is_none()
                {
                    preview.load_error = Some("Skill bundle is missing required files.".to_string());
                }
            }
            Err(error) => {
                preview.load_error = Some(error);
            }
        }

        previews.push(preview);
    }

    Ok(VerifiedSkillsListResponse {
        channel: registry.channel,
        skills: previews,
        installed: installed_state.skills,
    })
}

#[tauri::command]
pub fn skills_install_verified(app: AppHandle, skill_id: String) -> Result<InstalledSkillRecord, String> {
    let repo_root = find_repo_root()?;
    let registry = read_registry(&repo_root)?;

    let entry = registry
        .skills
        .iter()
        .find(|skill| skill.id == skill_id)
        .cloned()
        .ok_or_else(|| "Skill not found in verified registry.".to_string())?;

    let bundle_dir = resolve_bundle_dir(&repo_root, &entry.path)?;

    let manifest_path = bundle_dir.join("manifest.json");
    let skill_md_path = bundle_dir.join("SKILL.md");
    let prompt_md_path = bundle_dir.join("PROMPT.md");
    let icon_path = bundle_dir.join("icon.png");

    if !manifest_path.exists() || !skill_md_path.exists() || !prompt_md_path.exists() {
        return Err("Skill bundle is missing required files for install.".to_string());
    }

    let manifest_raw = fs::read_to_string(&manifest_path)
        .map_err(|_| "Unable to read manifest.json for install.".to_string())?;

    let manifest_value: Value =
        serde_json::from_str(&manifest_raw).map_err(|_| "manifest.json is invalid JSON.".to_string())?;

    let version = manifest_value
        .get("version")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "manifest.json is missing version.".to_string())?
        .to_string();

    let app_data = app_data_dir(&app)?;
    fs::create_dir_all(&app_data).map_err(|_| "Unable to initialize app data directory.".to_string())?;

    let install_dir = app_data
        .join("skills")
        .join("installed")
        .join(&entry.id)
        .join(&version);

    fs::create_dir_all(&install_dir)
        .map_err(|_| "Unable to create installed skill directory.".to_string())?;

    fs::write(install_dir.join("manifest.json"), &manifest_raw)
        .map_err(|_| "Unable to write installed manifest.json.".to_string())?;

    let skill_md_raw =
        fs::read_to_string(&skill_md_path).map_err(|_| "Unable to read SKILL.md for install.".to_string())?;
    fs::write(install_dir.join("SKILL.md"), skill_md_raw)
        .map_err(|_| "Unable to write installed SKILL.md.".to_string())?;

    let prompt_md_raw =
        fs::read_to_string(&prompt_md_path).map_err(|_| "Unable to read PROMPT.md for install.".to_string())?;
    fs::write(install_dir.join("PROMPT.md"), prompt_md_raw)
        .map_err(|_| "Unable to write installed PROMPT.md.".to_string())?;

    copy_if_exists(&icon_path, &install_dir.join("icon.png"))?;

    let installed_record = InstalledSkillRecord {
        id: entry.id,
        version,
        installed_at: now_unix_seconds_string()?,
        channel: registry.channel,
        install_dir: install_dir.to_string_lossy().to_string(),
    };

    let mut state = load_installed_state(&app)?;
    if let Some(existing) = state
        .skills
        .iter_mut()
        .find(|item| item.id == installed_record.id && item.version == installed_record.version)
    {
        *existing = installed_record.clone();
    } else {
        state.skills.push(installed_record.clone());
    }

    save_installed_state(&app, &state)?;

    Ok(installed_record)
}
