use base64::{engine::general_purpose::STANDARD, Engine as _};
use quick_xml::de::from_str as from_xml_str;
use reqwest::blocking::Client;
#[cfg(not(windows))]
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

mod crash_helper;
mod modpack;
mod world_backup;

const LAUNCHER_NAME: &str = "Bee Launcher";
const LAUNCHER_VERSION: &str = env!("CARGO_PKG_VERSION");
#[cfg(windows)]
const WINDOWS_APP_USER_MODEL_ID: &str = "com.blockline.launcher";
const BUNDLED_AUTHLIB_COORDINATE: &str = "com.mojang:authlib:10.0.77";
const BUNDLED_AUTHLIB_PATH: &str = "libraries/com/mojang/authlib/10.0.77/authlib-10.0.77.jar";
const BUNDLED_AUTHLIB_SIZE: u64 = 152_645;
const BUNDLED_AUTHLIB_SHA1: &str = "add8754cda96cf0cd840441632875648836f8a71";
const BUNDLED_AUTHLIB_BYTES: &[u8] = include_bytes!("../resources/authlib-10.0.77.jar");

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstalledVersion {
    profile_key: String,
    id: String,
    kind: String,
    instance_name: Option<String>,
    base_version: Option<String>,
    release_time: String,
    launchable: bool,
    required_java_major: Option<u32>,
    java_available: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppStatus {
    game_directory: String,
    installed_versions: Vec<InstalledVersion>,
    java_available: bool,
    java_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MinecraftNewsArticle {
    title: String,
    summary: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct MinecraftRssFeed {
    channel: MinecraftRssChannel,
}

#[derive(Debug, Deserialize)]
struct MinecraftRssChannel {
    #[serde(rename = "item", default)]
    items: Vec<MinecraftRssItem>,
}

#[derive(Debug, Default, Deserialize)]
struct MinecraftRssItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedMinecraftNews {
    articles: Vec<MinecraftNewsArticle>,
    fetched_at: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MinecraftNewsResponse {
    articles: Vec<MinecraftNewsArticle>,
    cached: bool,
    fetched_at: u64,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchResult {
    pid: Option<u32>,
    version_id: String,
    log_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CustomSkin {
    id: String,
    name: String,
    data_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkinLibrary {
    active_skin_id: Option<String>,
    skins: Vec<CustomSkin>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CustomSkinSupport {
    supported: bool,
    reason: String,
    loader_version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkinApplyResult {
    username: String,
    profile_id: String,
    replaced_existing: bool,
    restart_required: bool,
    config_backup_created: bool,
    skin_path: String,
    loader_config_path: String,
    staged_file_verified: bool,
    load_order_ready: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkinApplicationVerification {
    username: String,
    profile_id: String,
    skin_path: String,
    loader_log_path: String,
    file_matches_library: bool,
    load_order_ready: bool,
    loader_log_found: bool,
    loader_log_updated_after_skin: bool,
    matching_skin_fingerprint: bool,
    load_confirmed: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
struct StoredSettings {
    game_directory: Option<String>,
    game_directory_manual: bool,
    max_memory_gb: u8,
    width: u32,
    height: u32,
    fullscreen: bool,
    wallpaper_path: Option<String>,
    active_skin_id: Option<String>,
    profile_settings: HashMap<String, ProfileLaunchSettings>,
    graphics_restore_points: HashMap<String, Option<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
struct ProfileLaunchSettings {
    java_path: Option<String>,
    max_memory_gb: Option<u8>,
    graphics_preset: String,
}

impl Default for ProfileLaunchSettings {
    fn default() -> Self {
        Self {
            java_path: None,
            max_memory_gb: None,
            graphics_preset: "default".to_string(),
        }
    }
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            game_directory: None,
            game_directory_manual: false,
            max_memory_gb: 4,
            width: 1280,
            height: 720,
            fullscreen: false,
            wallpaper_path: None,
            active_skin_id: None,
            profile_settings: HashMap::new(),
            graphics_restore_points: HashMap::new(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherSettings {
    game_directory: String,
    max_memory_gb: u8,
    width: u32,
    height: u32,
    fullscreen: bool,
    wallpaper_path: Option<String>,
    wallpaper_name: Option<String>,
    profile_settings: HashMap<String, ProfileLaunchSettings>,
}

fn ensure_version_id(version_id: &str) -> Result<(), String> {
    if version_id.is_empty()
        || version_id == "."
        || version_id == ".."
        || version_id.trim() != version_id
        || version_id.len() > 150
        || !version_id.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '.' | '_' | '-' | ' ' | '+' | '(' | ')' | '[' | ']')
        })
    {
        return Err("That installed version name is not valid.".to_string());
    }
    Ok(())
}

fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!(
            "A version profile contains an unsafe file path: {relative}"
        ));
    }
    Ok(root.join(path))
}

fn candidate_game_dirs() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(windows)]
    {
        // Prefer the user's requested LocalAppData location, but discover the
        // official Launcher's existing Roaming folder as well.
        if let Some(local) = env::var_os("LOCALAPPDATA") {
            paths.push(PathBuf::from(local).join(".minecraft"));
        }
        if let Some(roaming) = env::var_os("APPDATA") {
            paths.push(PathBuf::from(roaming).join(".minecraft"));
        }
        if let Some(profile) = env::var_os("USERPROFILE") {
            paths.push(PathBuf::from(profile).join(".minecraft"));
        }
    }
    #[cfg(not(windows))]
    if let Some(home) = env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".minecraft"));
    }
    paths
}

fn default_game_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(local) = env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local).join(".minecraft");
        }
        if let Some(roaming) = env::var_os("APPDATA") {
            return PathBuf::from(roaming).join(".minecraft");
        }
        if let Some(profile) = env::var_os("USERPROFILE") {
            return PathBuf::from(profile).join(".minecraft");
        }
    }
    #[cfg(not(windows))]
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".minecraft");
    }
    PathBuf::from(".minecraft")
}

fn game_directory_score(path: &Path) -> (usize, usize) {
    if !path.is_dir() {
        return (0, 0);
    }
    let mut score = 0;
    for (directory, points) in [
        ("versions", 80),
        ("libraries", 24),
        ("assets", 24),
        ("runtime", 18),
        ("saves", 12),
        ("mods", 12),
        (".fabric", 18),
        ("resourcepacks", 6),
    ] {
        if path.join(directory).is_dir() {
            score += points;
        }
    }
    for (marker, points) in [
        ("TlauncherProfiles.json", 90),
        ("TLauncherProfiles.json", 90),
        ("TLauncher.exe", 90),
        ("launcher_profiles.json", 35),
        ("usercache.json", 8),
    ] {
        if path.join(marker).is_file() {
            score += points;
        }
    }

    let mut profile_count = 0;
    if let Ok(entries) = fs::read_dir(path.join("versions")) {
        for entry in entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
        {
            profile_count += 1;
            score += 5;
            let id = entry.file_name().to_string_lossy().into_owned();
            if entry.path().join(format!("{id}.json")).is_file() {
                score += 15;
            }
            if entry.path().join(format!("{id}.jar")).is_file() {
                score += 8;
            }
        }
    }
    let recognized_count = installed_versions(path).len();
    score += recognized_count * 40;
    (score, profile_count)
}

fn config_file(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("Could not find the launcher settings folder: {error}"))?;
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create launcher settings folder: {error}"))?;
    Ok(directory.join("settings.json"))
}

fn read_stored_settings(app: &AppHandle) -> StoredSettings {
    config_file(app)
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|contents| serde_json::from_str::<StoredSettings>(&contents).ok())
        .unwrap_or_default()
}

fn write_stored_settings(app: &AppHandle, settings: &StoredSettings) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("Could not encode launcher settings: {error}"))?;
    fs::write(config_file(app)?, bytes)
        .map_err(|error| format!("Could not save launcher settings: {error}"))
}

fn resolve_game_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let mut settings = read_stored_settings(app);
    if let Some(saved) = settings.game_directory.as_ref().map(PathBuf::from) {
        if saved.is_dir() && settings.game_directory_manual {
            return Ok(saved);
        }
    }

    let mut candidates = candidate_game_dirs();
    if let Some(saved) = settings.game_directory.as_ref().map(PathBuf::from) {
        if saved.is_dir() && !candidates.iter().any(|candidate| candidate == &saved) {
            candidates.push(saved);
        }
    }
    let mut existing: Vec<((usize, usize, usize), PathBuf)> = candidates
        .iter()
        .enumerate()
        .filter(|(_, path)| path.is_dir())
        .map(|(priority, path)| {
            let (score, profile_count) = game_directory_score(path);
            ((score, profile_count, usize::MAX - priority), path.clone())
        })
        .collect();
    existing.sort_by(|a, b| b.0.cmp(&a.0));
    let selected = existing
        .first()
        .map(|entry| entry.1.clone())
        .unwrap_or_else(default_game_dir);
    fs::create_dir_all(&selected)
        .map_err(|error| format!("Could not create the Minecraft folder: {error}"))?;
    let selected_text = selected.to_string_lossy().into_owned();
    if settings.game_directory.as_deref() != Some(selected_text.as_str())
        || settings.game_directory_manual
    {
        settings.game_directory = Some(selected_text);
        settings.game_directory_manual = false;
        write_stored_settings(app, &settings)?;
    }
    Ok(selected)
}

fn metadata_at(game_dir: &Path, version_id: &str) -> Option<Value> {
    let directory = game_dir.join("versions").join(version_id);
    let expected = directory.join(format!("{version_id}.json"));
    let path = if expected.is_file() {
        expected
    } else {
        fs::read_dir(&directory)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })?
    };
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn metadata_chain(game_dir: &Path, version_id: &str) -> Result<Vec<(String, Value)>, String> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = version_id.to_string();
    for _ in 0..16 {
        ensure_version_id(&current)?;
        if !seen.insert(current.clone()) {
            return Err("This version profile contains a circular parent reference.".to_string());
        }
        let metadata = metadata_at(game_dir, &current)
            .ok_or_else(|| format!("Could not read the installed profile {current}."))?;
        let parent = metadata
            .get("inheritsFrom")
            .and_then(Value::as_str)
            .map(str::to_string);
        chain.push((current.clone(), metadata));
        match parent {
            Some(parent_id) if parent_id != current => current = parent_id,
            _ => break,
        }
    }
    Ok(chain)
}

fn version_jar(game_dir: &Path, version_id: &str) -> Option<PathBuf> {
    let directory = game_dir.join("versions").join(version_id);
    let expected = directory.join(format!("{version_id}.jar"));
    if expected.is_file() {
        return Some(expected);
    }
    fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|extension| extension == "jar"))
}

fn version_has_jar(game_dir: &Path, version_id: &str) -> bool {
    let Ok(chain) = metadata_chain(game_dir, version_id) else {
        return version_jar(game_dir, version_id).is_some();
    };
    chain
        .iter()
        .any(|(id, _)| version_jar(game_dir, id).is_some())
}

fn profile_kind(version_id: &str, metadata: &Value) -> String {
    let name = version_id.to_ascii_lowercase();
    let libraries = metadata.get("libraries").and_then(Value::as_array);
    let library_text = libraries
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let combined = format!("{name} {library_text}");
    if combined.contains("fabric") {
        "Fabric".to_string()
    } else if combined.contains("quilt") {
        "Quilt".to_string()
    } else if combined.contains("forge") || combined.contains("minecraftforge") {
        "Forge".to_string()
    } else if combined.contains("neoforge") {
        "NeoForge".to_string()
    } else if combined.contains("optifine") {
        "OptiFine".to_string()
    } else if metadata.get("inheritsFrom").is_some() {
        "Modded".to_string()
    } else {
        "Vanilla".to_string()
    }
}

fn installed_versions(game_dir: &Path) -> Vec<InstalledVersion> {
    let Ok(entries) = fs::read_dir(game_dir.join("versions")) else {
        return Vec::new();
    };
    let mut versions: Vec<InstalledVersion> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            let metadata = metadata_at(game_dir, &id)?;
            let chain = metadata_chain(game_dir, &id).unwrap_or_default();
            let required_java = chain.iter().find_map(|(_, item)| required_java_major(item));
            let java_available = find_java_with_major(game_dir, required_java).is_some();
            let has_main_class = metadata.get("mainClass").and_then(Value::as_str).is_some()
                || chain
                    .iter()
                    .any(|(_, item)| item.get("mainClass").and_then(Value::as_str).is_some());
            let downloadable_client = chain.iter().any(|(_, item)| {
                item.pointer("/downloads/client/url")
                    .and_then(Value::as_str)
                    .is_some_and(|url| url.starts_with("https://"))
            });
            let launchable =
                has_main_class && (version_has_jar(game_dir, &id) || downloadable_client);
            let base_version = metadata
                .get("inheritsFrom")
                .and_then(Value::as_str)
                .map(str::to_string);
            let release_time = metadata
                .get("releaseTime")
                .or_else(|| metadata.get("time"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Some(InstalledVersion {
                profile_key: format!("minecraft:{id}"),
                kind: profile_kind(&id, &metadata),
                id,
                instance_name: None,
                base_version,
                release_time,
                launchable,
                required_java_major: required_java,
                java_available,
            })
        })
        .collect();
    versions.sort_by(|a, b| {
        b.release_time
            .cmp(&a.release_time)
            .then_with(|| b.id.cmp(&a.id))
    });
    versions
}

fn collect_java_candidates(root: &Path, depth: usize, candidates: &mut Vec<PathBuf>) {
    if depth > 8 || candidates.len() >= 256 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_java_candidates(&path, depth + 1, candidates);
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        #[cfg(windows)]
        let is_java = name == "java.exe" || name == "javaw.exe";
        #[cfg(not(windows))]
        let is_java = name == "java";
        if is_java {
            candidates.push(path);
        }
    }
}

fn java_major_from_output(output: &str) -> Option<u32> {
    let version = output
        .find('"')
        .and_then(|start| {
            output[start + 1..]
                .find('"')
                .map(|end| &output[start + 1..start + 1 + end])
        })
        .or_else(|| {
            output
                .split_whitespace()
                .find(|part| part.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        })?;
    let mut parts = version.split('.');
    let first = parts.next()?.trim_matches(|ch: char| !ch.is_ascii_digit());
    let major = first.parse::<u32>().ok()?;
    if major == 1 {
        parts.next()?.parse::<u32>().ok()
    } else {
        Some(major)
    }
}

#[cfg(windows)]
fn suppress_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    command.creation_flags(CREATE_NO_WINDOW);
}

fn java_major_version(candidate: &Path) -> Option<u32> {
    let mut command = Command::new(candidate);
    command.arg("-version");
    #[cfg(windows)]
    suppress_console_window(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    java_major_from_output(&text)
}

fn java_probe_path(candidate: &Path) -> PathBuf {
    #[cfg(windows)]
    if candidate
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("javaw.exe"))
    {
        let console_runtime = candidate.with_file_name("java.exe");
        if console_runtime.is_file() {
            return console_runtime;
        }
    }
    candidate.to_path_buf()
}

#[allow(unused_mut)]
fn first_usable_java(mut candidates: Vec<PathBuf>, required_major: Option<u32>) -> Option<PathBuf> {
    let mut seen = HashSet::new();
    #[cfg(windows)]
    candidates.sort_by_key(|candidate| {
        candidate
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("javaw.exe"))
    });
    candidates.into_iter().find(|candidate| {
        if !seen.insert(candidate.clone()) {
            return false;
        }
        java_major_version(candidate)
            .is_some_and(|major| required_major.is_none_or(|required| major == required))
    })
}

fn required_java_major(profile: &Value) -> Option<u32> {
    let version = profile.pointer("/javaVersion/majorVersion")?;
    version
        .as_u64()
        .or_else(|| version.as_f64().map(|major| major as u64))
        .and_then(|major| u32::try_from(major).ok())
}

fn find_java_for_profile(game_dir: &Path, profile: &Value) -> Option<PathBuf> {
    let required_major = required_java_major(profile);
    let component = profile
        .pointer("/javaVersion/component")
        .and_then(Value::as_str)
        .filter(|component| {
            !component.is_empty()
                && component
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        });
    let Some(component) = component else {
        return find_java_with_major(game_dir, required_major);
    };

    let mut runtime_roots = vec![game_dir.join("runtime").join(component)];
    if let Some(roaming) = env::var_os("APPDATA") {
        let roaming = PathBuf::from(roaming);
        runtime_roots.push(roaming.join(".minecraft/runtime").join(component));
        runtime_roots.push(roaming.join(".tlauncher/jvms").join(component));
    }
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        runtime_roots.push(local.join(".minecraft/runtime").join(component));
        runtime_roots.push(local.join(".tlauncher/jvms").join(component));
    }
    let mut candidates = Vec::new();
    for root in runtime_roots {
        collect_java_candidates(&root, 0, &mut candidates);
    }
    first_usable_java(candidates, required_major)
        .or_else(|| find_java_with_major(game_dir, required_major))
}

fn find_java(game_dir: &Path) -> Option<PathBuf> {
    find_java_with_major(game_dir, None)
}

fn java_for_game_launch(java: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        if java
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("java.exe"))
        {
            let gui_runtime = java.with_file_name("javaw.exe");
            if gui_runtime.is_file() {
                return gui_runtime;
            }
        }
    }
    java.to_path_buf()
}

fn find_java_with_major(game_dir: &Path, required_major: Option<u32>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = env::var_os("JAVA_HOME") {
        let java_home = PathBuf::from(home).join("bin");
        #[cfg(windows)]
        {
            candidates.push(java_home.join("javaw.exe"));
            candidates.push(java_home.join("java.exe"));
        }
        #[cfg(not(windows))]
        candidates.push(java_home.join("java"));
    }

    let mut runtime_roots = vec![game_dir.join("runtime")];
    if let Some(roaming) = env::var_os("APPDATA") {
        let roaming = PathBuf::from(roaming);
        runtime_roots.push(roaming.join(".minecraft/runtime"));
        runtime_roots.push(roaming.join(".tlauncher/jvms"));
    }
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        runtime_roots.push(local.join(".minecraft/runtime"));
        runtime_roots.push(local.join(".tlauncher/jvms"));
        runtime_roots.push(local.join("Programs/TLauncher/runtime"));
    }
    for root in runtime_roots {
        collect_java_candidates(&root, 0, &mut candidates);
    }

    #[cfg(windows)]
    {
        candidates.push(PathBuf::from("javaw.exe"));
        candidates.push(PathBuf::from("java.exe"));
    }
    #[cfg(not(windows))]
    candidates.push(PathBuf::from("java"));

    first_usable_java(candidates, required_major)
}

fn make_status(app: &AppHandle, game_dir: &Path) -> AppStatus {
    let java = find_java(game_dir);
    let mut versions = installed_versions(game_dir);
    versions.extend(modpack::installed_profiles(app));
    AppStatus {
        game_directory: game_dir.to_string_lossy().into_owned(),
        installed_versions: versions,
        java_available: java.is_some(),
        java_path: java.map(|path| path.to_string_lossy().into_owned()),
    }
}

#[tauri::command]
fn app_status(app: AppHandle) -> Result<AppStatus, String> {
    let game_dir = resolve_game_dir(&app)?;
    Ok(make_status(&app, &game_dir))
}

#[tauri::command]
fn set_game_directory(app: AppHandle, path: String) -> Result<AppStatus, String> {
    let selected = PathBuf::from(path);
    if selected.as_os_str().is_empty() {
        return Err("Choose a folder first.".to_string());
    }
    fs::create_dir_all(&selected)
        .map_err(|error| format!("Could not use that Minecraft folder: {error}"))?;
    let mut settings = read_stored_settings(&app);
    settings.game_directory = Some(selected.to_string_lossy().into_owned());
    settings.game_directory_manual = true;
    write_stored_settings(&app, &settings)?;
    Ok(make_status(&app, &selected))
}

fn launcher_settings(app: &AppHandle) -> Result<LauncherSettings, String> {
    let game_dir = resolve_game_dir(app)?;
    let stored = read_stored_settings(app);
    let wallpaper_name = stored
        .wallpaper_path
        .as_ref()
        .and_then(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned());
    Ok(LauncherSettings {
        game_directory: game_dir.to_string_lossy().into_owned(),
        max_memory_gb: stored.max_memory_gb,
        width: stored.width,
        height: stored.height,
        fullscreen: stored.fullscreen,
        wallpaper_path: stored.wallpaper_path,
        wallpaper_name,
        profile_settings: stored.profile_settings,
    })
}

#[tauri::command]
fn get_launcher_settings(app: AppHandle) -> Result<LauncherSettings, String> {
    launcher_settings(&app)
}

#[tauri::command]
fn save_launcher_settings(
    app: AppHandle,
    max_memory_gb: u8,
    width: u32,
    height: u32,
    fullscreen: bool,
) -> Result<LauncherSettings, String> {
    if !(1..=16).contains(&max_memory_gb) {
        return Err("Maximum memory must be between 1 and 16 GB.".to_string());
    }
    if !(800..=7680).contains(&width) || !(600..=4320).contains(&height) {
        return Err("Resolution must be between 800×600 and 7680×4320.".to_string());
    }
    let mut stored = read_stored_settings(&app);
    stored.max_memory_gb = max_memory_gb;
    stored.width = width;
    stored.height = height;
    stored.fullscreen = fullscreen;
    write_stored_settings(&app, &stored)?;
    launcher_settings(&app)
}

#[tauri::command]
fn save_profile_launch_settings(
    app: AppHandle,
    profile_key: String,
    profile_settings: ProfileLaunchSettings,
) -> Result<LauncherSettings, String> {
    if profile_settings
        .max_memory_gb
        .is_some_and(|memory| !(1..=16).contains(&memory))
    {
        return Err("Per-profile memory must be between 1 and 16 GB.".to_string());
    }
    if !matches!(
        profile_settings.graphics_preset.as_str(),
        "default" | "intel-uhd-low"
    ) {
        return Err("That graphics preset is not supported.".to_string());
    }
    let (profile_dir, version_id) = modpack::resolve_profile(&app, &profile_key)?;
    let chain = metadata_chain(&profile_dir, &version_id)?;
    let required_major = chain
        .iter()
        .find_map(|(_, metadata)| required_java_major(metadata));
    if let Some(path) = profile_settings.java_path.as_deref() {
        let java = PathBuf::from(path);
        if !java.is_file() {
            return Err("That Java executable no longer exists. Choose another Java runtime or use automatic detection.".to_string());
        }
        let actual_major = java_major_version(&java_probe_path(&java))
            .ok_or_else(|| "Bee Launcher could not read the selected Java version.".to_string())?;
        if required_major.is_some_and(|required| required != actual_major) {
            return Err(format!(
                "This profile requires Java {}, but the selected runtime is Java {actual_major}.",
                required_major.unwrap()
            ));
        }
    }
    let mut stored = read_stored_settings(&app);
    if profile_settings == ProfileLaunchSettings::default() {
        stored.profile_settings.remove(&profile_key);
    } else {
        stored
            .profile_settings
            .insert(profile_key, profile_settings);
    }
    write_stored_settings(&app, &stored)?;
    launcher_settings(&app)
}

fn wallpaper_data_url(path: &Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => return Err("Wallpaper must be a PNG, JPG, or WebP image.".to_string()),
    };
    let size = fs::metadata(path)
        .map_err(|error| format!("Could not read wallpaper details: {error}"))?
        .len();
    if size > 12 * 1024 * 1024 {
        return Err("Choose a wallpaper image smaller than 12 MB.".to_string());
    }
    let bytes =
        fs::read(path).map_err(|error| format!("Could not read wallpaper image: {error}"))?;
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}

#[tauri::command]
fn set_wallpaper(app: AppHandle, path: String) -> Result<String, String> {
    let image = PathBuf::from(path);
    if !image.is_file() {
        return Err("That wallpaper image could not be found.".to_string());
    }
    let data_url = wallpaper_data_url(&image)?;
    let mut stored = read_stored_settings(&app);
    stored.wallpaper_path = Some(image.to_string_lossy().into_owned());
    write_stored_settings(&app, &stored)?;
    Ok(data_url)
}

#[tauri::command]
fn get_wallpaper_data(app: AppHandle) -> Result<Option<String>, String> {
    let stored = read_stored_settings(&app);
    match stored.wallpaper_path {
        Some(path) if Path::new(&path).is_file() => Ok(Some(wallpaper_data_url(Path::new(&path))?)),
        _ => Ok(None),
    }
}

#[tauri::command]
fn read_local_image(path: String) -> Result<String, String> {
    let image = PathBuf::from(path);
    if !image.is_file() {
        return Err("That profile picture could not be found.".to_string());
    }
    wallpaper_data_url(&image).map_err(|error| {
        error
            .replace("Wallpaper", "Image")
            .replace("wallpaper", "image")
    })
}

fn custom_skin_directory(app: &AppHandle) -> Result<PathBuf, String> {
    let config = config_file(app)?;
    let parent = config
        .parent()
        .ok_or_else(|| "Could not find the launcher data folder.".to_string())?;
    let directory = parent.join("custom-skins");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create the custom skins folder: {error}"))?;
    Ok(directory)
}

fn safe_skin_id(id: &str) -> bool {
    id.starts_with("skin-")
        && id.ends_with(".png")
        && id.len() <= 120
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn skin_file(directory: &Path, id: &str) -> Result<PathBuf, String> {
    if !safe_skin_id(id) {
        return Err("That custom skin selection is not valid.".to_string());
    }
    Ok(directory.join(id))
}

fn read_skin_png(path: &Path) -> Result<Vec<u8>, String> {
    if !path.is_file() {
        return Err("That skin image could not be found.".to_string());
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if !extension.eq_ignore_ascii_case("png") {
        return Err("Minecraft skins must be PNG images.".to_string());
    }
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Could not read skin image details: {error}"))?;
    if metadata.len() > 2 * 1024 * 1024 {
        return Err("Choose a skin PNG smaller than 2 MB.".to_string());
    }
    let bytes = fs::read(path).map_err(|error| format!("Could not read skin image: {error}"))?;
    let decoder = png::Decoder::new(io::Cursor::new(bytes.as_slice()));
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("That file is not a valid Minecraft skin PNG: {error}"))?;
    let width = reader.info().width;
    let height = reader.info().height;
    if width != 64 || !matches!(height, 64 | 32) {
        return Err("Skin PNGs must be 64×64, or the older 64×32 format.".to_string());
    }
    let output_size = reader
        .output_buffer_size()
        .ok_or_else(|| "That skin PNG is too large to decode.".to_string())?;
    let mut decoded = vec![0; output_size];
    reader
        .next_frame(&mut decoded)
        .map_err(|error| format!("Could not decode that skin PNG: {error}"))?;
    Ok(bytes)
}

fn custom_skin_from_file(path: &Path, id: String) -> Result<CustomSkin, String> {
    let bytes = read_skin_png(path)?;
    let name_part = id
        .strip_prefix("skin-")
        .and_then(|value| value.split_once('-').map(|(_, name)| name))
        .and_then(|value| value.strip_suffix(".png"))
        .unwrap_or("custom");
    let name = name_part
        .replace('-', " ")
        .replace('_', " ")
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(CustomSkin {
        id,
        name: if name.is_empty() {
            "Custom skin".to_string()
        } else {
            name
        },
        data_url: format!("data:image/png;base64,{}", STANDARD.encode(bytes)),
    })
}

#[tauri::command]
fn list_custom_skins(app: AppHandle) -> Result<SkinLibrary, String> {
    let directory = custom_skin_directory(&app)?;
    let stored = read_stored_settings(&app);
    let mut skins = fs::read_dir(&directory)
        .map_err(|error| format!("Could not list custom skins: {error}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !entry.file_type().ok()?.is_file()
                || !path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            {
                return None;
            }
            let id = path.file_name()?.to_str()?.to_string();
            if !safe_skin_id(&id) {
                return None;
            }
            custom_skin_from_file(&path, id).ok()
        })
        .collect::<Vec<_>>();
    skins.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    let active_skin_id = stored
        .active_skin_id
        .filter(|id| safe_skin_id(id) && directory.join(id).is_file());
    Ok(SkinLibrary {
        active_skin_id,
        skins,
    })
}

#[tauri::command]
fn import_custom_skin(app: AppHandle, path: String) -> Result<CustomSkin, String> {
    let source = PathBuf::from(path);
    let bytes = read_skin_png(&source)?;
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("custom");
    let slug = stem
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
        })
        .take(48)
        .collect::<String>();
    let slug = if slug.is_empty() {
        "custom"
    } else {
        slug.as_str()
    };
    let directory = custom_skin_directory(&app)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut id = format!("skin-{timestamp}-{slug}.png");
    let mut suffix = 0_u32;
    while directory.join(&id).exists() {
        suffix += 1;
        id = format!("skin-{timestamp}-{suffix}-{slug}.png");
    }
    let destination = skin_file(&directory, &id)?;
    fs::write(&destination, &bytes)
        .map_err(|error| format!("Could not save this skin in Bee Launcher: {error}"))?;
    let mut stored = read_stored_settings(&app);
    stored.active_skin_id = Some(id.clone());
    if let Err(error) = write_stored_settings(&app, &stored) {
        let _ = fs::remove_file(&destination);
        return Err(error);
    }
    custom_skin_from_file(&destination, id)
}

#[tauri::command]
fn set_active_custom_skin(app: AppHandle, skin_id: Option<String>) -> Result<(), String> {
    let directory = custom_skin_directory(&app)?;
    if let Some(id) = skin_id.as_deref() {
        let path = skin_file(&directory, id)?;
        if !path.is_file() {
            return Err("That custom skin is no longer available.".to_string());
        }
        read_skin_png(&path)?;
    }
    let mut stored = read_stored_settings(&app);
    stored.active_skin_id = skin_id;
    write_stored_settings(&app, &stored)
}

#[tauri::command]
fn remove_custom_skin(app: AppHandle, skin_id: String) -> Result<(), String> {
    let directory = custom_skin_directory(&app)?;
    let path = skin_file(&directory, &skin_id)?;
    if !path.is_file() {
        return Err("That custom skin is no longer available.".to_string());
    }
    let mut stored = read_stored_settings(&app);
    if stored.active_skin_id.as_deref() == Some(skin_id.as_str()) {
        stored.active_skin_id = None;
        write_stored_settings(&app, &stored)?;
    }
    fs::remove_file(path).map_err(|error| format!("Could not remove this custom skin: {error}"))
}

fn parse_custom_skin_loader_version(version: &str) -> Option<(u32, u32, u32)> {
    let mut numbers = version
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u32>().ok());
    Some((
        numbers.next()?,
        numbers.next().unwrap_or(0),
        numbers.next().unwrap_or(0),
    ))
}

fn installed_custom_skin_loader_version(game_dir: &Path) -> Option<String> {
    let mods = game_dir.join("mods");
    let mut versions = fs::read_dir(mods)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
        })
        .filter_map(|entry| {
            let file = fs::File::open(entry.path()).ok()?;
            let mut archive = zip::ZipArchive::new(file).ok()?;
            let mut manifest = String::new();
            archive
                .by_name("fabric.mod.json")
                .ok()?
                .read_to_string(&mut manifest)
                .ok()?;
            let metadata: Value = serde_json::from_str(&manifest).ok()?;
            let id = metadata.get("id")?.as_str()?;
            if !id.starts_with("customskinloader") {
                return None;
            }
            let version = metadata.get("version")?.as_str()?.to_string();
            Some((parse_custom_skin_loader_version(&version)?, version))
        })
        .collect::<Vec<_>>();
    versions.sort_by(|left, right| right.0.cmp(&left.0));
    versions.into_iter().next().map(|(_, version)| version)
}

fn custom_skin_support_for_profile(game_dir: &Path, version_id: &str) -> CustomSkinSupport {
    let Some(profile) = installed_versions(game_dir)
        .into_iter()
        .find(|profile| profile.id == version_id)
    else {
        return CustomSkinSupport {
            supported: false,
            reason:
                "The selected profile could not be read. Rescan the Minecraft folder and try again."
                    .to_string(),
            loader_version: None,
        };
    };
    if !profile.kind.eq_ignore_ascii_case("fabric") {
        return CustomSkinSupport {
            supported: false,
            reason: "In-game local skins require the selected Fabric profile.".to_string(),
            loader_version: None,
        };
    }
    if profile.base_version.as_deref() != Some("26.3") {
        return CustomSkinSupport {
            supported: false,
            reason: "In-game skin support is enabled for Minecraft 26.3 Fabric profiles."
                .to_string(),
            loader_version: None,
        };
    }
    let Some(loader_version) = installed_custom_skin_loader_version(game_dir) else {
        return CustomSkinSupport {
            supported: false,
            reason: "CustomSkinLoader is not installed in this profile's mods folder. Bee Launcher will not install or change mods.".to_string(),
            loader_version: None,
        };
    };
    let compatible = parse_custom_skin_loader_version(&loader_version)
        .is_some_and(|version| version >= (15, 1, 0));
    CustomSkinSupport {
        supported: compatible,
        reason: if compatible {
            "CustomSkinLoader is ready. Applying a skin adds its local PNG and ensures LocalSkin is first in the load order, backing up an existing config before changing it.".to_string()
        } else {
            format!("CustomSkinLoader {loader_version} was found, but Minecraft 26.3 needs version 15.1 or newer. Bee Launcher will not replace or update the mod.")
        },
        loader_version: Some(loader_version),
    }
}

#[derive(Debug, Default)]
struct LocalSkinConfigUpdate {
    restart_required: bool,
    backup_created: bool,
}

fn default_local_skin_profile() -> Value {
    serde_json::json!({
        "name": "LocalSkin",
        "type": "Legacy",
        "checkPNG": false,
        "skin": "LocalSkin/skins/{USERNAME}.png",
        "model": "auto",
        "cape": "LocalSkin/capes/{USERNAME}.png",
        "elytra": "LocalSkin/elytras/{USERNAME}.png"
    })
}

fn prioritize_local_skin_profile(config: &mut Value) -> Result<bool, String> {
    let root = config.as_object_mut().ok_or_else(|| {
        "CustomSkinLoader's config is not a JSON object; it was left unchanged.".to_string()
    })?;
    let list = root
        .entry("loadlist".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if list.is_null() {
        *list = Value::Array(Vec::new());
    }
    let profiles = list.as_array_mut().ok_or_else(|| {
        "CustomSkinLoader's loadlist is not a JSON array; its config was left unchanged."
            .to_string()
    })?;
    let original = profiles.clone();
    let local_index = profiles.iter().position(|profile| {
        profile
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.eq_ignore_ascii_case("LocalSkin"))
    });
    let mut local = local_index
        .map(|index| profiles.remove(index))
        .unwrap_or_else(default_local_skin_profile);
    let local_object = local.as_object_mut().ok_or_else(|| {
        "CustomSkinLoader's LocalSkin entry is invalid; its config was left unchanged.".to_string()
    })?;
    local_object.insert("name".to_string(), Value::String("LocalSkin".to_string()));
    local_object.insert("type".to_string(), Value::String("Legacy".to_string()));
    local_object.insert(
        "skin".to_string(),
        Value::String("LocalSkin/skins/{USERNAME}.png".to_string()),
    );
    local_object
        .entry("model".to_string())
        .or_insert_with(|| Value::String("auto".to_string()));
    local_object
        .entry("checkPNG".to_string())
        .or_insert(Value::Bool(false));
    local_object
        .entry("cape".to_string())
        .or_insert_with(|| Value::String("LocalSkin/capes/{USERNAME}.png".to_string()));
    local_object
        .entry("elytra".to_string())
        .or_insert_with(|| Value::String("LocalSkin/elytras/{USERNAME}.png".to_string()));
    profiles.insert(0, local);
    Ok(*profiles != original)
}

fn bee_launcher_extra_local_skin_profile() -> Value {
    serde_json::json!({
        "name": "BeeLauncherLocalSkin",
        "type": "Legacy",
        "checkPNG": false,
        "skin": "LocalSkin/skins/{USERNAME}.png",
        "model": "auto",
        "cape": "BeeLauncherLocalSkin/capes/{USERNAME}.png",
        "elytra": "BeeLauncherLocalSkin/elytras/{USERNAME}.png"
    })
}

fn has_pending_local_skin_extra_list(directory: &Path) -> bool {
    fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_file()))
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .any(|entry| {
            fs::read(entry.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|profile| {
                    profile.get("name").and_then(Value::as_str) == Some("BeeLauncherLocalSkin")
                        && profile.get("type").and_then(Value::as_str) == Some("Legacy")
                        && profile.get("skin").and_then(Value::as_str)
                            == Some("LocalSkin/skins/{USERNAME}.png")
                })
        })
}

fn stage_local_skin_extra_list(game_dir: &Path) -> Result<(), String> {
    let extra_list = game_dir.join("CustomSkinLoader").join("ExtraList");
    fs::create_dir_all(&extra_list).map_err(|error| {
        format!("Could not prepare CustomSkinLoader's ExtraList folder: {error}")
    })?;
    if has_pending_local_skin_extra_list(&extra_list) {
        return Ok(());
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = extra_list.join(format!("bee-launcher-localskin-{timestamp}.json"));
    let bytes = serde_json::to_vec_pretty(&bee_launcher_extra_local_skin_profile())
        .map_err(|error| format!("Could not encode CustomSkinLoader's LocalSkin entry: {error}"))?;
    fs::write(path, bytes)
        .map_err(|error| format!("Could not stage CustomSkinLoader's LocalSkin priority: {error}"))
}

fn backup_custom_skin_loader_config(
    backup_directory: &Path,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    fs::create_dir_all(backup_directory).map_err(|error| {
        format!("Could not create a safe CustomSkinLoader config backup folder: {error}")
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let backup = backup_directory.join(format!("customskinloader-config-{timestamp}.json"));
    fs::write(&backup, bytes).map_err(|error| {
        format!("Could not back up CustomSkinLoader's config; it was left unchanged: {error}")
    })?;
    Ok(backup)
}

fn write_custom_skin_loader_config(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_file_name(format!(".CustomSkinLoader-{timestamp}.tmp"));
    fs::write(&temporary, bytes).map_err(|error| {
        format!("Could not prepare the CustomSkinLoader config update: {error}")
    })?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "Could not safely update CustomSkinLoader's config: {error}"
        ));
    }
    Ok(())
}

fn ensure_custom_skin_loader_local_skin_first(
    game_dir: &Path,
    backup_directory: &Path,
) -> Result<LocalSkinConfigUpdate, String> {
    let loader_directory = game_dir.join("CustomSkinLoader");
    let config_path = loader_directory.join("CustomSkinLoader.json");
    match fs::symlink_metadata(&config_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            stage_local_skin_extra_list(game_dir)?;
            Ok(LocalSkinConfigUpdate {
                restart_required: true,
                backup_created: false,
            })
        }
        Err(error) => Err(format!(
            "Could not inspect CustomSkinLoader's config: {error}"
        )),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            "CustomSkinLoader's config is not a regular file; it was left unchanged.".to_string(),
        ),
        Ok(metadata) if metadata.len() > 8 * 1024 * 1024 => Err(
            "CustomSkinLoader's config is larger than expected, so it was left unchanged."
                .to_string(),
        ),
        Ok(_) => {
            let original = fs::read(&config_path)
                .map_err(|error| format!("Could not read CustomSkinLoader's config: {error}"))?;
            let mut config: Value = serde_json::from_slice(&original).map_err(|error| {
                format!("CustomSkinLoader's config is not valid JSON, so it was left unchanged: {error}")
            })?;
            if !prioritize_local_skin_profile(&mut config)? {
                return Ok(LocalSkinConfigUpdate::default());
            }
            let backup = backup_custom_skin_loader_config(backup_directory, &original)?;
            let mut updated = serde_json::to_vec_pretty(&config)
                .map_err(|error| format!("Could not encode CustomSkinLoader's config: {error}"))?;
            updated.push(b'\n');
            write_custom_skin_loader_config(&config_path, &updated)
                .map_err(|error| format!("{error}. A backup was saved at {}.", backup.display()))?;
            Ok(LocalSkinConfigUpdate {
                restart_required: true,
                backup_created: true,
            })
        }
    }
}

fn custom_skin_profile_matches_local_skin(profile: &Value) -> bool {
    profile
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case("Legacy"))
        && profile.get("skin").and_then(Value::as_str) == Some("LocalSkin/skins/{USERNAME}.png")
}

fn local_skin_extra_list_is_ready(directory: &Path) -> bool {
    fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_file()))
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .any(|entry| {
            fs::metadata(entry.path())
                .ok()
                .is_some_and(|metadata| metadata.len() <= 1024 * 1024)
                && fs::read(entry.path())
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .is_some_and(|profile| {
                        profile.get("name").and_then(Value::as_str) == Some("BeeLauncherLocalSkin")
                            && custom_skin_profile_matches_local_skin(&profile)
                    })
        })
}

fn local_skin_priority_ready(game_dir: &Path) -> Result<bool, String> {
    let config_path = game_dir
        .join("CustomSkinLoader")
        .join("CustomSkinLoader.json");
    let extra_list = game_dir.join("CustomSkinLoader").join("ExtraList");
    match fs::symlink_metadata(&config_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err("CustomSkinLoader's config is not a regular file.".to_string())
        }
        Ok(metadata) if metadata.len() > 8 * 1024 * 1024 => {
            Err("CustomSkinLoader's config is larger than expected.".to_string())
        }
        Ok(_) => {
            let bytes = fs::read(&config_path)
                .map_err(|error| format!("Could not verify CustomSkinLoader's config: {error}"))?;
            let config: Value = serde_json::from_slice(&bytes).map_err(|error| {
                format!("Could not verify CustomSkinLoader's config JSON: {error}")
            })?;
            let first_profile_ready = config
                .get("loadlist")
                .and_then(Value::as_array)
                .and_then(|profiles| profiles.first())
                .is_some_and(custom_skin_profile_matches_local_skin);
            Ok(first_profile_ready || local_skin_extra_list_is_ready(&extra_list))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(local_skin_extra_list_is_ready(&extra_list))
        }
        Err(error) => Err(format!(
            "Could not inspect CustomSkinLoader's config: {error}"
        )),
    }
}

#[tauri::command]
fn get_custom_skin_support(
    app: AppHandle,
    profile_key: String,
) -> Result<CustomSkinSupport, String> {
    let (game_dir, version_id) = modpack::resolve_profile(&app, &profile_key)?;
    Ok(custom_skin_support_for_profile(&game_dir, &version_id))
}

#[tauri::command]
fn apply_custom_skin_to_profile(
    app: AppHandle,
    profile_key: String,
    username: String,
    skin_id: String,
) -> Result<SkinApplyResult, String> {
    validate_username(&username)?;
    let (game_dir, version_id) = modpack::resolve_profile(&app, &profile_key)?;
    let support = custom_skin_support_for_profile(&game_dir, &version_id);
    if !support.supported {
        return Err(support.reason);
    }
    let library = custom_skin_directory(&app)?;
    let source = skin_file(&library, &skin_id)?;
    let skin_bytes = read_skin_png(&source)?;
    let skin_directory = game_dir
        .join("CustomSkinLoader")
        .join("LocalSkin")
        .join("skins");
    fs::create_dir_all(&skin_directory).map_err(|error| {
        format!("Could not prepare CustomSkinLoader's local skin folder: {error}")
    })?;
    let target = skin_directory.join(format!("{username}.png"));
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let backup = match fs::symlink_metadata(&target) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(
                    "Bee Launcher will not replace a non-regular file in the LocalSkin folder."
                        .to_string(),
                );
            }
            if metadata.len() > 8 * 1024 * 1024 {
                return Err("The existing LocalSkin file is too large to back up safely, so it was left unchanged.".to_string());
            }
            let backup_directory = config_file(&app)?
                .parent()
                .ok_or_else(|| "Could not find the launcher data folder.".to_string())?
                .join("custom-skin-backups");
            fs::create_dir_all(&backup_directory)
                .map_err(|error| format!("Could not create a safe backup folder: {error}"))?;
            let backup_path =
                backup_directory.join(format!("local-skin-{timestamp}-{username}.png"));
            fs::copy(&target, &backup_path).map_err(|error| {
                format!("Could not back up the existing LocalSkin; it was left unchanged: {error}")
            })?;
            Some(backup_path)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("Could not inspect the existing LocalSkin: {error}")),
    };
    let temporary = skin_directory.join(format!(".bee-skin-{timestamp}.tmp"));
    fs::write(&temporary, &skin_bytes)
        .map_err(|error| format!("Could not prepare the local skin: {error}"))?;
    let settings_path = config_file(&app)?;
    let backup_directory = settings_path
        .parent()
        .ok_or_else(|| "Could not find the launcher data folder.".to_string())?
        .join("custom-skin-backups");
    let loader_config =
        match ensure_custom_skin_loader_local_skin_first(&game_dir, &backup_directory) {
            Ok(update) => update,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
    let load_order_ready = match local_skin_priority_ready(&game_dir) {
        Ok(ready) => ready,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    if !load_order_ready {
        let _ = fs::remove_file(&temporary);
        return Err("The LocalSkin profile could not be verified at the start of CustomSkinLoader's load order; the skin was not installed.".to_string());
    }
    if backup.is_some() {
        fs::remove_file(&target)
            .map_err(|error| format!("Could not replace the existing LocalSkin: {error}"))?;
    }
    if let Err(error) = fs::rename(&temporary, &target) {
        let _ = fs::remove_file(&temporary);
        if let Some(backup_path) = backup.as_ref() {
            let _ = fs::copy(backup_path, &target);
        }
        return Err(format!("Could not install the local skin: {error}"));
    }
    let staged_file_verified = fs::read(&target)
        .map(|written| written == skin_bytes)
        .unwrap_or(false);
    if !staged_file_verified {
        if let Some(backup_path) = backup.as_ref() {
            let _ = fs::copy(backup_path, &target);
        } else {
            let _ = fs::remove_file(&target);
        }
        return Err("The staged skin could not be verified against the selected library image; the previous local file was restored if one existed.".to_string());
    }
    Ok(SkinApplyResult {
        username,
        profile_id: version_id,
        replaced_existing: backup.is_some(),
        restart_required: loader_config.restart_required,
        config_backup_created: loader_config.backup_created,
        skin_path: target.to_string_lossy().to_string(),
        loader_config_path: game_dir
            .join("CustomSkinLoader")
            .join("CustomSkinLoader.json")
            .to_string_lossy()
            .to_string(),
        staged_file_verified,
        load_order_ready,
    })
}

fn modified_epoch_millis(path: &Path) -> Option<u128> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

fn local_skin_loader_fingerprint(path: &Path, relative_path: &str) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    // CustomSkinLoader 15.x hashes the decimal file size + configured relative path +
    // Java File.lastModified() in milliseconds for its LOCAL_LEGACY texture URL.
    let input = format!("{}{relative_path}{modified}", metadata.len());
    Some(format!("{:x}", Sha1::digest(input.as_bytes())))
}

fn bounded_skin_log_tail(path: &Path, maximum: u64) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(maximum)))
        .ok()?;
    let mut bytes = Vec::with_capacity(length.min(maximum) as usize);
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn skin_loader_log_has_fingerprint(log: &str, fingerprint: &str, relative_path: &str) -> bool {
    let expected = format!("(LOCAL_LEGACY){fingerprint},{relative_path}").to_ascii_lowercase();
    log.to_ascii_lowercase().contains(&expected)
}

#[tauri::command]
fn verify_custom_skin_application(
    app: AppHandle,
    profile_key: String,
    username: String,
    skin_id: String,
) -> Result<SkinApplicationVerification, String> {
    validate_username(&username)?;
    let (game_dir, profile_id) = modpack::resolve_profile(&app, &profile_key)?;
    let library = custom_skin_directory(&app)?;
    let source = skin_file(&library, &skin_id)?;
    let expected_skin = read_skin_png(&source)?;
    let relative_path = format!("LocalSkin/skins/{username}.png");
    let skin_path = game_dir
        .join("CustomSkinLoader")
        .join("LocalSkin")
        .join("skins")
        .join(format!("{username}.png"));
    let skin_metadata = fs::symlink_metadata(&skin_path).ok();
    let file_matches_library = skin_metadata.as_ref().is_some_and(|metadata| {
        !metadata.file_type().is_symlink()
            && metadata.is_file()
            && metadata.len() <= 8 * 1024 * 1024
            && fs::read(&skin_path).is_ok_and(|bytes| bytes == expected_skin)
    });
    let load_order_ready = local_skin_priority_ready(&game_dir)?;
    let loader_log_path = game_dir
        .join("CustomSkinLoader")
        .join("CustomSkinLoader.log");
    let log_metadata = fs::symlink_metadata(&loader_log_path).ok();
    let loader_log_found = log_metadata
        .as_ref()
        .is_some_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_file());
    let skin_modified = modified_epoch_millis(&skin_path);
    let log_modified = modified_epoch_millis(&loader_log_path);
    let loader_log_updated_after_skin = loader_log_found
        && skin_modified
            .zip(log_modified)
            .is_some_and(|(skin, log)| log > skin);
    let fingerprint = if file_matches_library {
        local_skin_loader_fingerprint(&skin_path, &relative_path)
    } else {
        None
    };
    let matching_skin_fingerprint = loader_log_updated_after_skin
        && fingerprint.as_deref().is_some_and(|fingerprint| {
            bounded_skin_log_tail(&loader_log_path, 128 * 1024).is_some_and(|log| {
                skin_loader_log_has_fingerprint(&log, fingerprint, &relative_path)
            })
        });
    let load_confirmed = file_matches_library
        && load_order_ready
        && loader_log_updated_after_skin
        && matching_skin_fingerprint;
    Ok(SkinApplicationVerification {
        username,
        profile_id,
        skin_path: skin_path.to_string_lossy().to_string(),
        loader_log_path: loader_log_path.to_string_lossy().to_string(),
        file_matches_library,
        load_order_ready,
        loader_log_found,
        loader_log_updated_after_skin,
        matching_skin_fingerprint,
        load_confirmed,
    })
}

#[tauri::command]
fn reset_wallpaper(app: AppHandle) -> Result<(), String> {
    let mut stored = read_stored_settings(&app);
    stored.wallpaper_path = None;
    write_stored_settings(&app, &stored)
}

fn current_os_name() -> &'static str {
    #[cfg(windows)]
    return "windows";
    #[cfg(target_os = "macos")]
    return "osx";
    #[cfg(target_os = "linux")]
    return "linux";
    #[allow(unreachable_code)]
    "unknown"
}

#[cfg(windows)]
fn current_os_version() -> Option<String> {
    static VERSION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    VERSION
        .get_or_init(|| {
            let mut command = Command::new("cmd.exe");
            command.args(["/C", "ver"]);
            suppress_console_window(&mut command);
            let output = command.output().ok()?;
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            text.split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
                .find(|part| {
                    part.contains('.')
                        && part
                            .split('.')
                            .all(|number| !number.is_empty() && number.parse::<u64>().is_ok())
                })
                .map(str::to_string)
        })
        .clone()
}

#[cfg(not(windows))]
fn current_os_version() -> Option<String> {
    None
}

fn compare_dotted_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    let left: Vec<u64> = left
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<_, _>>()
        .ok()?;
    let right: Vec<u64> = right
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<_, _>>()
        .ok()?;
    let length = left.len().max(right.len());
    for index in 0..length {
        let ordering = left
            .get(index)
            .copied()
            .unwrap_or(0)
            .cmp(&right.get(index).copied().unwrap_or(0));
        if ordering != std::cmp::Ordering::Equal {
            return Some(ordering);
        }
    }
    Some(std::cmp::Ordering::Equal)
}

fn version_in_range(current: &str, range: &Value) -> bool {
    if let Some(minimum) = range.get("min").and_then(Value::as_str) {
        if !matches!(
            compare_dotted_versions(current, minimum),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ) {
            return false;
        }
    }
    if let Some(maximum) = range.get("max").and_then(Value::as_str) {
        if !matches!(
            compare_dotted_versions(current, maximum),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ) {
            return false;
        }
    }
    true
}

fn rule_matches(rule: &Value, os_name: &str) -> bool {
    if let Some(os) = rule.get("os") {
        if let Some(name) = os.get("name").and_then(Value::as_str) {
            if name != os_name {
                return false;
            }
        }
        if let Some(arch) = os.get("arch").and_then(Value::as_str) {
            let actual = env::consts::ARCH;
            let matches = match arch {
                "x86" => actual == "x86" || actual == "i686",
                "x86_64" => actual == "x86_64",
                other => actual == other,
            };
            if !matches {
                return false;
            }
        }
        if let Some(range) = os.get("versionRange") {
            let Some(version) = current_os_version() else {
                return false;
            };
            if !version_in_range(&version, range) {
                return false;
            }
        }
    }
    // Optional feature-gated launcher arguments are absent in this local profile.
    if rule.get("features").is_some() {
        return false;
    }
    true
}

fn allowed_by_rules(value: &Value, os_name: &str) -> bool {
    let Some(rules) = value.get("rules").and_then(Value::as_array) else {
        return true;
    };
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        if rule_matches(rule, os_name) {
            allowed = rule.get("action").and_then(Value::as_str) == Some("allow");
        }
    }
    allowed
}

fn coordinate_path(name: &str) -> Option<String> {
    let (coordinate, extension) = name.split_once('@').unwrap_or((name, "jar"));
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let group_path = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let classifier = parts.get(3).copied();
    let suffix = classifier
        .map(|part| format!("-{part}"))
        .unwrap_or_default();
    Some(format!(
        "{group_path}/{artifact}/{version}/{artifact}-{version}{suffix}.{extension}"
    ))
}

#[derive(Clone, Debug)]
struct ArtifactSpec {
    relative_path: String,
    url: String,
    fallback_urls: Vec<String>,
    sha1: Option<String>,
    size: Option<u64>,
}

fn artifact_spec(
    value: &Value,
    fallback_path: Option<String>,
    fallback_url: Option<String>,
) -> Option<ArtifactSpec> {
    let relative_path = value
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(fallback_path)?;
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(fallback_url)?;
    Some(ArtifactSpec {
        relative_path,
        url,
        fallback_urls: Vec::new(),
        sha1: value
            .get("sha1")
            .and_then(Value::as_str)
            .map(str::to_string),
        size: value.get("size").and_then(Value::as_u64),
    })
}

fn minecraft_library_destination(path: &str) -> String {
    let path = path.trim_start_matches('/');
    if path.starts_with("libraries/") {
        path.to_string()
    } else {
        format!("libraries/{path}")
    }
}

fn library_artifact_spec(library: &Value) -> Option<ArtifactSpec> {
    let name = library.get("name").and_then(Value::as_str)?;
    let artifact_path = library
        .pointer("/downloads/artifact/path")
        .and_then(Value::as_str)
        .or_else(|| library.pointer("/artifact/path").and_then(Value::as_str))
        .map(str::to_string)
        .or_else(|| coordinate_path(name))?;
    let artifact = library
        .pointer("/downloads/artifact")
        .or_else(|| library.get("artifact"));
    let repo_url = library
        .get("url")
        .and_then(Value::as_str)
        .map(|base| {
            format!(
                "{}/{}",
                base.trim_end_matches('/'),
                artifact_path.trim_start_matches('/')
            )
        })
        .or_else(|| {
            Some(format!(
                "https://libraries.minecraft.net/{}",
                artifact_path.trim_start_matches('/')
            ))
        });
    let empty = Value::Null;
    let mut spec = artifact_spec(
        artifact.unwrap_or(&empty),
        Some(artifact_path.clone()),
        repo_url,
    )?;
    let fallback_path = artifact_path.trim_start_matches('/');
    spec.relative_path = minecraft_library_destination(&spec.relative_path);
    if !name.starts_with("com.mojang:") {
        for base in [
            "https://repo.maven.apache.org/maven2",
            "https://repo1.maven.org/maven2",
        ] {
            let mirror = format!("{base}/{fallback_path}");
            if mirror != spec.url && !spec.fallback_urls.contains(&mirror) {
                spec.fallback_urls.push(mirror);
            }
        }
    }
    Some(spec)
}

fn sha1_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|error| format!("Could not verify {}: {error}", path.display()))?;
    let mut digest = Sha1::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not verify {}: {error}", path.display()))?;
        if bytes == 0 {
            break;
        }
        digest.update(&buffer[..bytes]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn sha1_bytes(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn artifact_matches(path: &Path, expected_size: Option<u64>, expected_sha1: Option<&str>) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return false;
    }
    if expected_size.is_some_and(|size| metadata.len() != size) {
        return false;
    }
    if let Some(expected) = expected_sha1 {
        return sha1_file(path)
            .map(|actual| actual.eq_ignore_ascii_case(expected))
            .unwrap_or(false);
    }
    true
}

fn ensure_downloaded(
    client: &Client,
    game_dir: &Path,
    spec: &ArtifactSpec,
    label: &str,
) -> Result<PathBuf, String> {
    if !spec.url.starts_with("https://") {
        return Err(format!("Refusing a non-HTTPS download URL for {label}."));
    }
    let destination = safe_join(game_dir, &spec.relative_path)?;
    if artifact_matches(&destination, spec.size, spec.sha1.as_deref()) {
        return Ok(destination);
    }
    let parent = destination
        .parent()
        .ok_or_else(|| format!("Invalid destination for {label}."))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the folder for {label}: {error}"))?;
    let temporary = destination.with_extension("blockline-download");
    let urls = std::iter::once(&spec.url).chain(spec.fallback_urls.iter());
    let mut failures = Vec::new();
    for url in urls {
        if !url.starts_with("https://") {
            failures.push(format!("Refusing non-HTTPS source {url}"));
            continue;
        }
        let mut response = match client
            .get(url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
        {
            Ok(response) => response,
            Err(error) => {
                failures.push(format!("{url}: {error}"));
                continue;
            }
        };
        let result = (|| {
            let mut output = fs::File::create(&temporary).map_err(|error| {
                format!("Could not create a temporary file for {label}: {error}")
            })?;
            let byte_count = io::copy(&mut response, &mut output)
                .map_err(|error| format!("Could not save {label}: {error}"))?;
            output
                .sync_all()
                .map_err(|error| format!("Could not finish saving {label}: {error}"))?;
            if spec.size.is_some_and(|size| size != byte_count) {
                return Err(format!(
                    "Downloaded {label} has the wrong size ({byte_count} bytes)."
                ));
            }
            if !artifact_matches(&temporary, spec.size, spec.sha1.as_deref()) {
                return Err(format!("Downloaded {label} failed its SHA-1/size check."));
            }
            if destination.exists() {
                fs::remove_file(&destination).map_err(|error| {
                    format!("Could not replace the invalid local {label}: {error}")
                })?;
            }
            fs::rename(&temporary, &destination)
                .map_err(|error| format!("Could not install {label}: {error}"))?;
            Ok(())
        })();
        match result {
            Ok(()) => return Ok(destination),
            Err(error) => {
                failures.push(format!("{url}: {error}"));
                let _ = fs::remove_file(&temporary);
            }
        }
    }
    Err(format!(
        "Could not download {label} from any source. {}",
        failures.join(" | ")
    ))
}

fn downloader() -> Result<Client, String> {
    Client::builder()
        .user_agent(format!("{LAUNCHER_NAME}/{LAUNCHER_VERSION}"))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| format!("Could not initialize the game-file downloader: {error}"))
}

fn ensure_client_jar(
    client: &Client,
    game_dir: &Path,
    chain: &[(String, Value)],
) -> Result<(), String> {
    for (id, metadata) in chain {
        let Some(download) = metadata.pointer("/downloads/client") else {
            continue;
        };
        ensure_version_id(id)?;
        let relative = format!("versions/{id}/{id}.jar");
        let spec = artifact_spec(
            download,
            Some(relative),
            download
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
        .ok_or_else(|| format!("The installed profile {id} has incomplete client-JAR metadata."))?;
        ensure_downloaded(
            client,
            game_dir,
            &spec,
            &format!("Minecraft client JAR for {id}"),
        )?;
        return Ok(());
    }
    Ok(())
}

fn native_coordinate_matches(name: &str, os: &str, arch: &str) -> bool {
    let Some(classifier) = name.rsplit(':').next() else {
        return false;
    };
    let Some(platform) = classifier.strip_prefix("natives-") else {
        return false;
    };
    let parts: Vec<&str> = platform.split('-').collect();
    if parts.first().copied()
        != Some(match os {
            "windows" => "windows",
            "osx" => "macos",
            _ => "linux",
        })
    {
        return false;
    }
    if parts.len() == 1 {
        return true;
    }
    let suffix = parts[1..].join("-");
    match arch {
        "x86_64" => matches!(suffix.as_str(), "x86_64" | "x64"),
        "aarch64" => matches!(suffix.as_str(), "arm64" | "aarch64"),
        "x86" | "i386" | "i586" | "i686" => matches!(suffix.as_str(), "x86" | "i386"),
        _ => false,
    }
}

fn current_native_os_classifier(name: &str) -> bool {
    native_coordinate_matches(name, current_os_name(), env::consts::ARCH)
}

fn ensure_library_artifacts(
    client: &Client,
    game_dir: &Path,
    chain: &[(String, Value)],
) -> Result<(), String> {
    let mut prepared = HashSet::new();
    for (_, metadata) in chain {
        let Some(libraries) = metadata.get("libraries").and_then(Value::as_array) else {
            continue;
        };
        for library in libraries {
            if !allowed_by_rules(library, current_os_name()) {
                continue;
            }
            let name = library.get("name").and_then(Value::as_str).unwrap_or("");
            if name
                .rsplit(':')
                .next()
                .is_some_and(|part| part.starts_with("natives-"))
                && !current_native_os_classifier(name)
            {
                continue;
            }
            if let Some(spec) = library_artifact_spec(library) {
                if prepared.insert(spec.relative_path.clone()) {
                    ensure_downloaded(client, game_dir, &spec, &format!("library {}", name))?;
                }
            }
            if let Some((classifier, _)) = native_classifier(library) {
                if let Some(artifact) = library
                    .pointer("/downloads/classifiers")
                    .and_then(|classifiers| classifiers.get(&classifier))
                {
                    let mut spec = artifact_spec(artifact, None, None).ok_or_else(|| {
                        format!("Native library {name} has incomplete download metadata.")
                    })?;
                    spec.relative_path = minecraft_library_destination(&spec.relative_path);
                    if prepared.insert(spec.relative_path.clone()) {
                        ensure_downloaded(
                            client,
                            game_dir,
                            &spec,
                            &format!("native library {name}"),
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn bundled_authlib_spec(chain: &[(String, Value)]) -> Option<ArtifactSpec> {
    chain.iter().find_map(|(_, metadata)| {
        metadata
            .get("libraries")
            .and_then(Value::as_array)?
            .iter()
            .find(|library| {
                library.get("name").and_then(Value::as_str) == Some(BUNDLED_AUTHLIB_COORDINATE)
            })
            .and_then(library_artifact_spec)
    })
}

fn install_bundled_authlib(
    game_dir: &Path,
    chain: &[(String, Value)],
) -> Result<Option<PathBuf>, String> {
    let Some(spec) = bundled_authlib_spec(chain) else {
        return Ok(None);
    };
    if spec.relative_path != BUNDLED_AUTHLIB_PATH
        || spec.size != Some(BUNDLED_AUTHLIB_SIZE)
        || spec.sha1.as_deref() != Some(BUNDLED_AUTHLIB_SHA1)
    {
        return Err(
            "The bundled Mojang authlib does not match this profile's declared artifact; refusing to substitute a different JAR."
                .to_string(),
        );
    }
    if BUNDLED_AUTHLIB_BYTES.len() as u64 != BUNDLED_AUTHLIB_SIZE
        || !sha1_bytes(BUNDLED_AUTHLIB_BYTES).eq_ignore_ascii_case(BUNDLED_AUTHLIB_SHA1)
    {
        return Err("The bundled Mojang authlib failed its integrity check.".to_string());
    }

    let destination = safe_join(game_dir, &spec.relative_path)?;
    if artifact_matches(&destination, spec.size, spec.sha1.as_deref()) {
        return Ok(Some(destination));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "Invalid destination for the bundled Mojang authlib.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the Mojang authlib folder: {error}"))?;
    let temporary = destination.with_extension("blockline-bundled");
    fs::write(&temporary, BUNDLED_AUTHLIB_BYTES)
        .map_err(|error| format!("Could not install the bundled Mojang authlib: {error}"))?;
    if !artifact_matches(&temporary, spec.size, spec.sha1.as_deref()) {
        let _ = fs::remove_file(&temporary);
        return Err(
            "The bundled Mojang authlib failed the profile's SHA-1/size check.".to_string(),
        );
    }
    if destination.exists() {
        fs::remove_file(&destination).map_err(|error| {
            format!("Could not replace the invalid local Mojang authlib: {error}")
        })?;
    }
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("Could not install the bundled Mojang authlib: {error}"))?;
    Ok(Some(destination))
}

fn add_classpath_path(
    game_dir: &Path,
    relative: &str,
    paths: &mut Vec<String>,
    seen: &mut HashSet<String>,
) -> Result<(), String> {
    let path = safe_join(game_dir, relative)?;
    if !path.is_file() {
        return Err(format!(
            "A required Minecraft library is missing after preparation: {}",
            path.display()
        ));
    }
    let text = path.to_string_lossy().into_owned();
    if seen.insert(text.clone()) {
        paths.push(text);
    }
    Ok(())
}

fn native_classifier(library: &Value) -> Option<(String, Vec<String>)> {
    let natives_key = format!("natives.{}", current_os_name());
    let template = library
        .get("natives")
        .and_then(|value| value.get(natives_key))
        .and_then(Value::as_str)?;
    let arch = if cfg!(target_pointer_width = "64") {
        "64"
    } else {
        "32"
    };
    let classifier = template.replace("${arch}", arch);
    let excluded = library
        .pointer("/extract/exclude")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some((classifier, excluded))
}

fn extract_native_archive(
    archive_path: &Path,
    destination: &Path,
    excluded: &[String],
) -> Result<(), String> {
    let file = fs::File::open(archive_path).map_err(|error| {
        format!(
            "Could not open local native library {}: {error}",
            archive_path.display()
        )
    })?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("A local native library archive is invalid: {error}"))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not read a local native library: {error}"))?;
        let Some(relative) = entry.enclosed_name().map(PathBuf::from) else {
            continue;
        };
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            continue;
        }
        let relative_text = relative.to_string_lossy().replace('\\', "/");
        if excluded.iter().any(|item| relative_text.starts_with(item))
            || entry.is_dir()
            || !is_native_binary(&relative)
        {
            continue;
        }
        let output_path = destination.join(relative);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not prepare native library files: {error}"))?;
        }
        if output_path.is_file() {
            continue;
        }
        let mut output = fs::File::create(output_path)
            .map_err(|error| format!("Could not write native library files: {error}"))?;
        io::copy(&mut entry, &mut output)
            .map_err(|error| format!("Could not extract a native library: {error}"))?;
    }
    Ok(())
}

fn is_native_binary(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match current_os_name() {
        "windows" => extension == "dll",
        "osx" => matches!(extension.as_str(), "dylib" | "jnilib"),
        _ => extension == "so",
    }
}

fn native_files_exist(directory: &Path) -> bool {
    let Ok(entries) = fs::read_dir(directory) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            native_files_exist(&path)
        } else {
            is_native_binary(&path)
        }
    })
}

fn classpath_and_extract_natives(
    game_dir: &Path,
    version_id: &str,
    chain: &[(String, Value)],
) -> Result<(String, PathBuf), String> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    let mut native_archives = Vec::new();
    let mut native_seen = HashSet::new();
    for (_, metadata) in chain {
        let Some(libraries) = metadata.get("libraries").and_then(Value::as_array) else {
            continue;
        };
        for library in libraries {
            if !allowed_by_rules(library, current_os_name()) {
                continue;
            }
            let name = library.get("name").and_then(Value::as_str).unwrap_or("");
            let coordinate_native = name
                .rsplit(':')
                .next()
                .is_some_and(|part| part.starts_with("natives-"));
            if coordinate_native {
                if current_native_os_classifier(name) {
                    let spec = library_artifact_spec(library).ok_or_else(|| {
                        format!("Native library {name} has no usable local artifact path.")
                    })?;
                    let path = safe_join(game_dir, &spec.relative_path)?;
                    if !path.is_file() {
                        return Err(format!(
                            "Required native library is missing after preparation: {}",
                            path.display()
                        ));
                    }
                    if native_seen.insert(path.clone()) {
                        native_archives.push((path, Vec::new()));
                    }
                }
                continue;
            }
            if let Some(spec) = library_artifact_spec(library) {
                add_classpath_path(game_dir, &spec.relative_path, &mut paths, &mut seen)?;
            }

            if let Some((classifier, excluded)) = native_classifier(library) {
                let classifier_artifact = library
                    .pointer("/downloads/classifiers")
                    .and_then(|value| value.get(&classifier));
                if let Some(artifact) = classifier_artifact {
                    let spec = artifact_spec(artifact, None, None).ok_or_else(|| {
                        format!("Native classifier {classifier} has incomplete metadata.")
                    })?;
                    let path = safe_join(game_dir, &spec.relative_path)?;
                    if !path.is_file() {
                        return Err(format!(
                            "Required native library is missing after preparation: {}",
                            path.display()
                        ));
                    }
                    if native_seen.insert(path.clone()) {
                        native_archives.push((path, excluded));
                    }
                } else if let Some(name) = library.get("name").and_then(Value::as_str) {
                    let coordinate = name.split('@').next().unwrap_or(name);
                    let base = coordinate.split(':').take(3).collect::<Vec<_>>().join(":");
                    let native_name = format!("{base}:{classifier}");
                    if let Some(relative) = coordinate_path(&native_name) {
                        let path = safe_join(game_dir, &relative)?;
                        if !path.is_file() {
                            return Err(format!(
                                "Required native library is missing after preparation: {}",
                                path.display()
                            ));
                        }
                        if native_seen.insert(path.clone()) {
                            native_archives.push((path, excluded));
                        }
                    }
                }
            }
        }
    }

    let profile_dir = game_dir.join("versions").join(version_id);
    let existing_native_dir = profile_dir.join("natives");
    let native_dir = if existing_native_dir.is_dir() {
        existing_native_dir
    } else {
        profile_dir.join(format!("{version_id}-natives"))
    };
    fs::create_dir_all(&native_dir)
        .map_err(|error| format!("Could not create the local Java native folder: {error}"))?;
    let mut native_subdirectories = HashSet::new();
    for (_, metadata) in chain {
        for key in ["default_user_jvm", "jvm"] {
            for argument in argument_values(
                metadata.pointer(&format!("/arguments/{key}")),
                current_os_name(),
            ) {
                if let Some((_, remainder)) = argument.split_once("${natives_directory}/") {
                    let subdirectory = remainder.split(['/', '\\']).next().unwrap_or("");
                    if !subdirectory.is_empty()
                        && subdirectory
                            .chars()
                            .all(|character| character.is_ascii_alphanumeric() || character == '_')
                    {
                        native_subdirectories.insert(subdirectory.to_string());
                    }
                }
            }
        }
    }
    for subdirectory in &native_subdirectories {
        fs::create_dir_all(native_dir.join(subdirectory))
            .map_err(|error| format!("Could not prepare the native support folder: {error}"))?;
    }
    let extract_dir = if native_subdirectories.contains("java") {
        native_dir.join("java")
    } else {
        native_dir.clone()
    };
    for (archive, excluded) in native_archives {
        extract_native_archive(&archive, &extract_dir, &excluded)?;
    }

    let profile_uses_native_path = chain.iter().any(|(_, metadata)| {
        ["default_user_jvm", "jvm"].iter().any(|key| {
            argument_values(
                metadata.pointer(&format!("/arguments/{key}")),
                current_os_name(),
            )
            .iter()
            .any(|argument| argument.contains("${natives_directory}"))
        })
    });
    if profile_uses_native_path && !native_files_exist(&extract_dir) {
        return Err(format!(
            "No {} native DLLs were found or extracted for this profile. Check the libraries folder and retry.",
            current_os_name()
        ));
    }

    Ok((
        paths.join(if cfg!(windows) { ";" } else { ":" }),
        native_dir,
    ))
}

fn jopt_simple_spec(chain: &[(String, Value)]) -> Option<ArtifactSpec> {
    chain.iter().find_map(|(_, metadata)| {
        metadata
            .get("libraries")
            .and_then(Value::as_array)?
            .iter()
            .find(|library| {
                library
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.starts_with("net.sf.jopt-simple:jopt-simple:"))
            })
            .and_then(library_artifact_spec)
    })
}

fn include_jopt_simple_classpath(
    game_dir: &Path,
    chain: &[(String, Value)],
    library_classpath: &str,
) -> Result<(String, Option<PathBuf>), String> {
    let Some(spec) = jopt_simple_spec(chain) else {
        return Ok((library_classpath.to_string(), None));
    };
    let path = safe_join(game_dir, &spec.relative_path)?;
    let file = fs::File::open(&path).map_err(|error| {
        format!(
            "Could not open the required jopt-simple library {}: {error}",
            path.display()
        )
    })?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("The jopt-simple library is not a readable JAR: {error}"))?;
    if archive.by_name("joptsimple/ValueConverter.class").is_err() {
        return Err(format!(
            "The jopt-simple library {} is missing joptsimple/ValueConverter.class.",
            path.display()
        ));
    }

    let path_text = path.to_string_lossy();
    let separator = if cfg!(windows) { ';' } else { ':' };
    let already_included = library_classpath.split(separator).any(|entry| {
        if cfg!(windows) {
            entry.eq_ignore_ascii_case(path_text.as_ref())
        } else {
            entry == path_text.as_ref()
        }
    });
    let classpath = if already_included {
        library_classpath.to_string()
    } else if library_classpath.is_empty() {
        path_text.into_owned()
    } else {
        format!("{library_classpath}{separator}{path_text}")
    };
    Ok((classpath, Some(path)))
}

fn offline_uuid(username: &str) -> String {
    let mut bytes = md5::compute(format!("Offline:{username}")).0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn validate_username(username: &str) -> Result<(), String> {
    if !(3..=16).contains(&username.len())
        || !username
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err("Player name must be 3–16 letters, numbers, or underscores.".to_string());
    }
    Ok(())
}

fn argument_values(value: Option<&Value>, os_name: &str) -> Vec<String> {
    let Some(values) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for value in values {
        if let Some(text) = value.as_str() {
            result.push(text.to_string());
        } else if value.is_object() && allowed_by_rules(value, os_name) {
            match value.get("values").or_else(|| value.get("value")) {
                Some(Value::String(text)) => result.push(text.to_string()),
                Some(Value::Array(items)) => {
                    result.extend(items.iter().filter_map(Value::as_str).map(str::to_string))
                }
                _ => {}
            }
        }
    }
    result
}

fn replace_placeholders(value: &str, replacements: &[(String, String)]) -> String {
    replacements
        .iter()
        .fold(value.to_string(), |text, (key, replacement)| {
            text.replace(&format!("${{{key}}}"), replacement)
        })
}

fn first_field<'a>(chain: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    chain.iter().find_map(|(_, metadata)| metadata.get(key))
}

fn resolve_client_jar(
    game_dir: &Path,
    version_id: &str,
    chain: &[(String, Value)],
) -> Result<PathBuf, String> {
    ensure_version_id(version_id)?;
    for (id, _) in chain {
        ensure_version_id(id)?;
        if let Some(candidate) = version_jar(game_dir, id) {
            return Ok(candidate);
        }
    }
    Err(format!(
        "No client jar was found for {version_id}. Check that the version is fully installed."
    ))
}

fn set_game_option(args: &mut Vec<String>, option: &str, value: String) {
    let mut index = 0;
    while index < args.len() {
        if args[index] == option {
            args.remove(index);
            if index < args.len() {
                args.remove(index);
            }
        } else if args[index].starts_with(&format!("{option}=")) {
            args.remove(index);
        } else {
            index += 1;
        }
    }
    args.push(option.to_string());
    args.push(value);
}

fn log_excerpt(path: &Path) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return "(Java did not write a startup log.)".to_string();
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if length > 12_000 {
        let _ = file.seek(SeekFrom::Start(length - 12_000));
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() || bytes.is_empty() {
        return "(Java did not write a startup log.)".to_string();
    }
    format!("{}", String::from_utf8_lossy(&bytes))
}

fn fail_if_early_exit(child: &mut Child, log_path: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("Could not monitor the Minecraft process: {error}"))?
        {
            let code = status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            return Err(format!(
                "Minecraft exited during startup (exit code {code}). Log: {}\n{}",
                log_path.display(),
                log_excerpt(log_path)
            ));
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn write_launch_diagnostics(
    log: &mut fs::File,
    version_id: &str,
    game_dir: &Path,
    java: &Path,
    main_class: &str,
    classpath: &str,
    bundled_authlib_path: Option<&Path>,
    jopt_simple_path: Option<&Path>,
    jvm_args: &[String],
    game_args: &[String],
) -> io::Result<()> {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let classpath_entries: Vec<&str> = classpath
        .split(separator)
        .filter(|entry| !entry.is_empty())
        .collect();
    writeln!(log, "Bee Launcher {LAUNCHER_VERSION} launch diagnostics")?;
    writeln!(log, "Profile: {version_id}")?;
    writeln!(log, "Game directory: {}", game_dir.display())?;
    writeln!(log, "Java executable: {}", java.display())?;
    writeln!(log, "Host: {} / {}", current_os_name(), env::consts::ARCH)?;
    writeln!(log, "Main class: {main_class}")?;
    writeln!(log, "JVM arguments:")?;
    for argument in jvm_args {
        if argument == classpath {
            writeln!(
                log,
                "  <classpath: {} entries; listed below>",
                classpath_entries.len()
            )?;
        } else {
            writeln!(log, "  {argument}")?;
        }
    }
    writeln!(log, "Classpath entries: {}", classpath_entries.len())?;
    for entry in classpath_entries {
        writeln!(log, "  {entry}")?;
    }
    if let Some(path) = bundled_authlib_path {
        writeln!(
            log,
            "Mojang authlib 10.0.77: bundled artifact verified at {}",
            path.display()
        )?;
    } else {
        writeln!(log, "Mojang authlib 10.0.77: not required by this profile")?;
    }
    if let Some(path) = jopt_simple_path {
        writeln!(
            log,
            "jopt-simple ValueConverter: verified in {}; included in classpath",
            path.display()
        )?;
    } else {
        writeln!(log, "jopt-simple: not declared by this profile")?;
    }
    writeln!(log, "Game arguments:")?;
    for argument in game_args {
        writeln!(log, "  {argument}")?;
    }
    writeln!(log, "--- Java process output follows ---")?;
    log.flush()
}

fn apply_intel_uhd_low_graphics(game_dir: &Path) -> Result<(), String> {
    let options_path = game_dir.join("options.txt");
    let previous = fs::read_to_string(&options_path).unwrap_or_default();
    let overrides = [
        ("graphicsMode", "0"),
        ("renderDistance", "6"),
        ("simulationDistance", "4"),
        ("maxFps", "60"),
        ("clouds", "0"),
        ("particles", "1"),
        ("mipmapLevels", "0"),
        ("entityDistanceScaling", "0.5"),
        ("biomeBlendRadius", "0"),
        ("entityShadows", "false"),
        ("ao", "0"),
    ];
    let mut seen = HashSet::new();
    let mut lines = previous
        .lines()
        .map(|line| {
            if let Some((key, _)) = line.split_once(':') {
                if let Some((_, value)) = overrides
                    .iter()
                    .find(|(override_key, _)| *override_key == key)
                {
                    seen.insert(key.to_string());
                    return format!("{key}:{value}");
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>();
    for (key, value) in overrides {
        if !seen.contains(key) {
            lines.push(format!("{key}:{value}"));
        }
    }
    fs::write(&options_path, format!("{}\n", lines.join("\n")))
        .map_err(|error| format!("Could not apply the Intel UHD low-graphics preset: {error}"))
}

fn restore_graphics_baseline(game_dir: &Path, original: Option<&str>) -> Result<(), String> {
    let options_path = game_dir.join("options.txt");
    match original {
        Some(contents) => fs::write(&options_path, contents).map_err(|error| {
            format!("Could not restore the previous Minecraft graphics options: {error}")
        }),
        None => match fs::remove_file(&options_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "Could not restore the previous Minecraft graphics options: {error}"
            )),
        },
    }
}

fn launch_game(
    app: &AppHandle,
    game_dir: &Path,
    version_id: &str,
    profile_key: &str,
    username: &str,
) -> Result<LaunchResult, String> {
    ensure_version_id(version_id)?;
    validate_username(username)?;
    let chain = metadata_chain(game_dir, version_id)?;
    let java_profile = chain
        .iter()
        .find(|(_, metadata)| required_java_major(metadata).is_some())
        .map(|(_, metadata)| metadata)
        .unwrap_or(&chain[0].1);
    let required_major = required_java_major(java_profile);
    let mut launch_settings = read_stored_settings(app);
    let profile_settings = launch_settings
        .profile_settings
        .get(profile_key)
        .cloned()
        .unwrap_or_default();
    let java_runtime = if let Some(path) = profile_settings.java_path.as_deref() {
        let java = PathBuf::from(path);
        if !java.is_file() {
            return Err("The selected profile's Java runtime is missing. Choose another runtime in Settings or switch it back to automatic detection.".to_string());
        }
        let actual_major = java_major_version(&java_probe_path(&java)).ok_or_else(|| {
            "Bee Launcher could not read the selected profile's Java runtime.".to_string()
        })?;
        if required_major.is_some_and(|required| required != actual_major) {
            return Err(format!("This profile requires Java {}, but its saved runtime is Java {actual_major}. Update the Java selection in Settings.", required_major.unwrap()));
        }
        java
    } else {
        find_java_for_profile(game_dir, java_profile).ok_or_else(|| {
            required_major.map_or_else(
                || "A usable Java runtime was not found for this profile.".to_string(),
                |major| {
                    format!("This profile requires Java {major}; no compatible runtime was found.")
                },
            )
        })?
    };
    let java = java_for_game_launch(&java_runtime);
    let http = downloader()?;
    ensure_client_jar(&http, game_dir, &chain)?;
    let bundled_authlib_path = install_bundled_authlib(game_dir, &chain)?;
    ensure_library_artifacts(&http, game_dir, &chain)?;
    if let Some(spec) = jopt_simple_spec(&chain) {
        ensure_downloaded(&http, game_dir, &spec, "jopt-simple support library")?;
    }
    let graphics_restore_key = game_dir.to_string_lossy().into_owned();
    if profile_settings.graphics_preset == "intel-uhd-low" {
        if !launch_settings
            .graphics_restore_points
            .contains_key(&graphics_restore_key)
        {
            let original = fs::read_to_string(game_dir.join("options.txt")).ok();
            launch_settings
                .graphics_restore_points
                .insert(graphics_restore_key.clone(), original);
            write_stored_settings(app, &launch_settings)?;
        }
        apply_intel_uhd_low_graphics(game_dir)?;
    } else if let Some(original) = launch_settings
        .graphics_restore_points
        .remove(&graphics_restore_key)
    {
        restore_graphics_baseline(game_dir, original.as_deref())?;
        write_stored_settings(app, &launch_settings)?;
    }
    let client_jar = resolve_client_jar(game_dir, version_id, &chain)?;
    let (library_classpath, native_dir) =
        classpath_and_extract_natives(game_dir, version_id, &chain)?;
    let (library_classpath, jopt_simple_path) =
        include_jopt_simple_classpath(&game_dir, &chain, &library_classpath)?;
    let mut classpath_parts = Vec::new();
    if !library_classpath.is_empty() {
        classpath_parts.push(library_classpath);
    }
    classpath_parts.push(client_jar.to_string_lossy().into_owned());
    let classpath = classpath_parts.join(if cfg!(windows) { ";" } else { ":" });

    let assets_root = game_dir.join("assets").to_string_lossy().into_owned();
    let asset_index = first_field(&chain, "assetIndex")
        .and_then(|index| index.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("legacy")
        .to_string();
    let profile = &chain[0].1;
    let version_type = profile
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("release");
    let offline_id = offline_uuid(username);
    let replacements = vec![
        ("auth_player_name".to_string(), username.to_string()),
        ("version_name".to_string(), version_id.to_string()),
        (
            "game_directory".to_string(),
            game_dir.to_string_lossy().into_owned(),
        ),
        ("assets_root".to_string(), assets_root.clone()),
        ("game_assets".to_string(), assets_root),
        ("assets_index_name".to_string(), asset_index),
        ("auth_uuid".to_string(), offline_id),
        ("auth_access_token".to_string(), "0".to_string()),
        ("auth_session".to_string(), "0".to_string()),
        ("user_type".to_string(), "legacy".to_string()),
        ("version_type".to_string(), version_type.to_string()),
        ("clientid".to_string(), String::new()),
        ("auth_xuid".to_string(), String::new()),
        (
            "natives_directory".to_string(),
            native_dir.to_string_lossy().into_owned(),
        ),
        ("launcher_name".to_string(), LAUNCHER_NAME.to_string()),
        ("launcher_version".to_string(), LAUNCHER_VERSION.to_string()),
        ("classpath".to_string(), classpath.clone()),
        (
            "classpath_separator".to_string(),
            if cfg!(windows) { ";" } else { ":" }.to_string(),
        ),
        ("user_properties".to_string(), "{}".to_string()),
        (
            "resolution_width".to_string(),
            launch_settings.width.to_string(),
        ),
        (
            "resolution_height".to_string(),
            launch_settings.height.to_string(),
        ),
    ];

    let mut default_user_jvm_args = Vec::new();
    let mut jvm_args = Vec::new();
    let mut game_args = Vec::new();
    for (_, metadata) in &chain {
        if default_user_jvm_args.is_empty() {
            default_user_jvm_args = argument_values(
                metadata.pointer("/arguments/default_user_jvm"),
                current_os_name(),
            );
        }
        if jvm_args.is_empty() {
            jvm_args = argument_values(metadata.pointer("/arguments/jvm"), current_os_name());
        }
        if game_args.is_empty() {
            game_args = argument_values(metadata.pointer("/arguments/game"), current_os_name());
            if game_args.is_empty() {
                if let Some(legacy) = metadata.get("minecraftArguments").and_then(Value::as_str) {
                    game_args = legacy.split_whitespace().map(str::to_string).collect();
                }
            }
        }
    }
    default_user_jvm_args.extend(jvm_args);
    jvm_args = default_user_jvm_args;
    jvm_args = jvm_args
        .iter()
        .map(|arg| replace_placeholders(arg, &replacements))
        .collect();
    game_args = game_args
        .iter()
        .map(|arg| replace_placeholders(arg, &replacements))
        .collect();

    if !jvm_args
        .iter()
        .any(|arg| arg.starts_with("-Djava.library.path="))
    {
        jvm_args.push(format!(
            "-Djava.library.path={}",
            native_dir.to_string_lossy()
        ));
    }
    let has_classpath = jvm_args
        .iter()
        .any(|arg| arg == "-cp" || arg == "-classpath" || arg.contains(&classpath));
    if !has_classpath {
        jvm_args.push("-cp".to_string());
        jvm_args.push(classpath.clone());
    }
    jvm_args.retain(|arg| !arg.starts_with("-Xmx"));
    let memory_gb = profile_settings
        .max_memory_gb
        .unwrap_or(launch_settings.max_memory_gb);
    jvm_args.push(format!("-Xmx{memory_gb}G"));
    let main_class = first_field(&chain, "mainClass")
        .and_then(Value::as_str)
        .ok_or_else(|| "The selected profile is missing its Java main class.".to_string())?;

    if game_args.is_empty() {
        game_args = vec![
            "--username".to_string(),
            username.to_string(),
            "--version".to_string(),
            version_id.to_string(),
            "--gameDir".to_string(),
            game_dir.to_string_lossy().into_owned(),
            "--assetsDir".to_string(),
            game_dir.join("assets").to_string_lossy().into_owned(),
            "--assetIndex".to_string(),
            first_field(&chain, "assetIndex")
                .and_then(|index| index.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("legacy")
                .to_string(),
            "--uuid".to_string(),
            offline_uuid(username),
            "--accessToken".to_string(),
            "0".to_string(),
            "--userType".to_string(),
            "legacy".to_string(),
        ];
    }
    set_game_option(&mut game_args, "--width", launch_settings.width.to_string());
    set_game_option(
        &mut game_args,
        "--height",
        launch_settings.height.to_string(),
    );
    game_args.retain(|arg| arg != "--fullscreen" && arg != "--windowed");
    if launch_settings.fullscreen {
        game_args.push("--fullscreen".to_string());
    }

    let log_dir = game_dir.join("logs");
    fs::create_dir_all(&log_dir)
        .map_err(|error| format!("Could not create the launcher log folder: {error}"))?;
    let log_path = log_dir.join("blockline-latest.log");
    let mut log = fs::File::create(&log_path)
        .map_err(|error| format!("Could not create the game log: {error}"))?;
    write_launch_diagnostics(
        &mut log,
        version_id,
        &game_dir,
        &java,
        main_class,
        &classpath,
        bundled_authlib_path.as_deref(),
        jopt_simple_path.as_deref(),
        &jvm_args,
        &game_args,
    )
    .map_err(|error| format!("Could not write launch diagnostics: {error}"))?;
    let stdout = log
        .try_clone()
        .map_err(|error| format!("Could not open the game log: {error}"))?;
    let mut command = Command::new(&java);
    command
        .args(&jvm_args)
        .arg(main_class)
        .args(&game_args)
        .current_dir(&game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(log));
    #[cfg(windows)]
    suppress_console_window(&mut command);
    let mut child = command.spawn().map_err(|error| {
        format!(
            "Java could not start this local Minecraft profile: {error}. See {}",
            log_path.display()
        )
    })?;
    let pid = child.id();
    fail_if_early_exit(&mut child, &log_path)?;
    Ok(LaunchResult {
        pid: Some(pid),
        version_id: version_id.to_string(),
        log_path: log_path.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
fn launch_version(
    app: AppHandle,
    profile_key: String,
    username: String,
) -> Result<LaunchResult, String> {
    let (game_dir, version_id) = modpack::resolve_profile(&app, &profile_key)?;
    launch_game(&app, &game_dir, &version_id, &profile_key, &username)
}

#[tauri::command]
fn import_modpack(app: AppHandle, path: String) -> Result<modpack::ImportResult, String> {
    modpack::import_modpack(&app, &path)
}

#[tauri::command]
fn open_game_directory(app: AppHandle) -> Result<(), String> {
    let game_dir = resolve_game_dir(&app)?;
    #[cfg(windows)]
    let mut command = Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    command
        .arg(&game_dir)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open {}: {error}", game_dir.display()))
}

fn decode_article_text(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&#38;", "&")
}

fn plain_article_text(markup: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    for character in markup.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !output.ends_with(' ') {
                    output.push(' ');
                }
            }
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    decode_article_text(&output.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn official_article_url(href: &str) -> Option<String> {
    if href.starts_with("/") && !href.starts_with("//") {
        Some(format!("https://www.minecraft.net{href}"))
    } else if href.starts_with("https://www.minecraft.net/")
        || href.starts_with("https://minecraft.net/")
    {
        Some(href.to_string())
    } else {
        None
    }
}

fn parse_minecraft_news_feed(xml: &str) -> Result<Vec<MinecraftNewsArticle>, String> {
    let feed: MinecraftRssFeed = from_xml_str(xml)
        .map_err(|_| "The official Minecraft News feed format could not be read.".to_string())?;
    let mut articles = Vec::new();
    for item in feed.channel.items {
        let title = plain_article_text(&item.title);
        let summary = plain_article_text(&item.description);
        let Some(url) = official_article_url(item.link.trim()) else {
            continue;
        };
        if title.is_empty()
            || articles
                .iter()
                .any(|article: &MinecraftNewsArticle| article.url == url)
        {
            continue;
        }
        articles.push(MinecraftNewsArticle {
            title,
            summary,
            url,
        });
        if articles.len() >= 10 {
            break;
        }
    }
    if articles.is_empty() {
        return Err(
            "The official Minecraft News feed did not contain readable articles.".to_string(),
        );
    }
    Ok(articles)
}

fn minecraft_news_cache_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_cache_dir()
        .ok()
        .map(|directory| directory.join("minecraft-news.json"))
}

fn load_minecraft_news_cache(path: &Path) -> Option<CachedMinecraftNews> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let cache: CachedMinecraftNews = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    if cache.articles.is_empty()
        || cache.articles.iter().any(|article| {
            official_article_url(&article.url).as_deref() != Some(article.url.as_str())
        })
    {
        return None;
    }
    Some(cache)
}

fn save_minecraft_news_cache(path: &Path, cache: &CachedMinecraftNews) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Could not locate the news cache folder.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not prepare the news cache: {error}"))?;
    let contents = serde_json::to_vec(cache)
        .map_err(|error| format!("Could not encode the news cache: {error}"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_file_name(format!(
        ".minecraft-news-{}-{nonce}.tmp",
        std::process::id()
    ));
    let result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&contents)?;
        file.sync_all()?;
        replace_news_cache_file(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("Could not save the news cache: {error}"));
    }
    Ok(())
}

#[cfg(windows)]
fn replace_news_cache_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    let temporary: Vec<u16> = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect();
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_news_cache_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(temporary, destination)
}

const MINECRAFT_NEWS_FEED_URL: &str = "https://www.minecraft.net/en-us/feeds/community-content/rss";
const MAX_MINECRAFT_NEWS_BYTES: usize = 5 * 1024 * 1024;

fn valid_minecraft_news_feed_url(url: &str) -> bool {
    if url == MINECRAFT_NEWS_FEED_URL {
        return true;
    }
    #[cfg(test)]
    {
        return url.starts_with("http://127.0.0.1:") || url.starts_with("http://[::1]:");
    }
    #[cfg(not(test))]
    false
}

fn fetch_minecraft_news_feed(url: &str) -> Result<String, String> {
    if !valid_minecraft_news_feed_url(url) {
        return Err("Bee only requests the official Minecraft News feed.".to_string());
    }
    #[cfg(windows)]
    let body = fetch_minecraft_news_feed_with_wininet(url)?;
    #[cfg(not(windows))]
    let body = fetch_minecraft_news_feed_with_reqwest(url)?;
    if body.len() > MAX_MINECRAFT_NEWS_BYTES {
        return Err("The official Minecraft News feed was larger than expected.".to_string());
    }
    Ok(body)
}

#[cfg(not(windows))]
fn fetch_minecraft_news_feed_with_reqwest(url: &str) -> Result<String, String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .user_agent(format!("BeeLauncher/{LAUNCHER_VERSION}"))
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many official Minecraft News redirects")
            } else if attempt.previous().iter().any(|url| url == attempt.url()) {
                attempt.error("official Minecraft News redirect loop blocked")
            } else if attempt.url().scheme() != "https"
                || !matches!(
                    attempt.url().host_str(),
                    Some("www.minecraft.net" | "minecraft.net")
                )
            {
                attempt.error("redirect to a non-official Minecraft News host blocked")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|error| format!("Could not prepare the official news request: {error}"))?;
    let response = client
        .get(url)
        .header(
            reqwest::header::ACCEPT,
            "application/rss+xml, application/xml;q=0.9, text/xml;q=0.8",
        )
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .send()
        .map_err(|error| news_transport_error(&error))?;
    if !is_official_minecraft_news_response(response.url()) {
        return Err(
            "Minecraft News redirected away from the official website; the response was blocked."
                .to_string(),
        );
    }
    if !response.status().is_success() {
        return Err(format!(
            "The official Minecraft News feed returned HTTP {}.",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length as usize > MAX_MINECRAFT_NEWS_BYTES)
    {
        return Err("The official Minecraft News feed was larger than expected.".to_string());
    }
    response
        .text()
        .map_err(|error| news_transport_error(&error))
}

#[cfg(not(windows))]
fn is_official_minecraft_news_response(url: &reqwest::Url) -> bool {
    if url.scheme() == "https"
        && matches!(url.host_str(), Some("www.minecraft.net" | "minecraft.net"))
    {
        return true;
    }
    #[cfg(test)]
    {
        return url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "[::1]"));
    }
    #[cfg(not(test))]
    false
}

#[cfg(not(windows))]
fn news_transport_error(error: &reqwest::Error) -> String {
    let cause = if error.is_timeout() {
        "the request timed out"
    } else if error.is_connect() {
        "the connection failed"
    } else {
        "the request failed"
    };
    format!(
        "Bee could not retrieve the official Minecraft News feed because {cause}. Details: {error}"
    )
}

#[cfg(windows)]
fn fetch_minecraft_news_feed_with_wininet(url: &str) -> Result<String, String> {
    use std::{ffi::c_void, mem::size_of, ptr::null};
    use windows_sys::Win32::{
        Foundation::GetLastError,
        Networking::WinInet::{
            HttpQueryInfoW, InternetCloseHandle, InternetOpenUrlW, InternetOpenW, InternetReadFile,
            InternetSetOptionW, HTTP_QUERY_FLAG_NUMBER, HTTP_QUERY_STATUS_CODE,
            INTERNET_FLAG_NO_AUTO_REDIRECT, INTERNET_FLAG_NO_CACHE_WRITE, INTERNET_FLAG_NO_UI,
            INTERNET_FLAG_RELOAD, INTERNET_FLAG_SECURE, INTERNET_OPEN_TYPE_PRECONFIG,
            INTERNET_OPTION_CONNECT_TIMEOUT, INTERNET_OPTION_RECEIVE_TIMEOUT,
        },
    };

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                InternetCloseHandle(self.0);
            }
        }
    }
    fn wininet_error(stage: &str) -> String {
        let code = unsafe { GetLastError() };
        format!("Bee could not retrieve the official Minecraft News feed ({stage}; Windows error {code}).")
    }

    let agent: Vec<u16> = format!("BeeLauncher/{LAUNCHER_VERSION}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let url_wide: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
    let headers: Vec<u16> = "Accept: application/rss+xml, application/xml;q=0.9, text/xml;q=0.8\r\nAccept-Language: en-US,en;q=0.9\r\n".encode_utf16().chain(Some(0)).collect();
    let session = unsafe {
        InternetOpenW(
            agent.as_ptr(),
            INTERNET_OPEN_TYPE_PRECONFIG,
            null(),
            null(),
            0,
        )
    };
    if session.is_null() {
        return Err(wininet_error("opening Windows Internet settings"));
    }
    let session = Handle(session);
    for (option, milliseconds) in [
        (INTERNET_OPTION_CONNECT_TIMEOUT, 10_000u32),
        (INTERNET_OPTION_RECEIVE_TIMEOUT, 20_000u32),
    ] {
        let applied = unsafe {
            InternetSetOptionW(
                session.0,
                option,
                &milliseconds as *const _ as *const c_void,
                size_of::<u32>() as u32,
            )
        };
        if applied == 0 {
            return Err(wininet_error("setting a bounded network timeout"));
        }
    }
    let security_flag = if url.starts_with("https://") {
        INTERNET_FLAG_SECURE
    } else {
        0
    };
    let request = unsafe {
        InternetOpenUrlW(
            session.0,
            url_wide.as_ptr(),
            headers.as_ptr(),
            headers.len().saturating_sub(1) as u32,
            INTERNET_FLAG_RELOAD
                | INTERNET_FLAG_NO_CACHE_WRITE
                | INTERNET_FLAG_NO_UI
                | INTERNET_FLAG_NO_AUTO_REDIRECT
                | security_flag,
            0,
        )
    };
    if request.is_null() {
        return Err(wininet_error("opening the official feed URL"));
    }
    let request = Handle(request);
    let mut status = 0u32;
    let mut status_len = size_of::<u32>() as u32;
    let mut index = 0u32;
    let status_read = unsafe {
        HttpQueryInfoW(
            request.0,
            HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER,
            &mut status as *mut _ as *mut c_void,
            &mut status_len,
            &mut index,
        )
    };
    if status_read == 0 {
        return Err(wininet_error("reading the official feed response status"));
    }
    if !(200..300).contains(&status) {
        return Err(format!(
            "The official Minecraft News feed returned HTTP {status}."
        ));
    }
    let mut output = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let mut bytes_read = 0u32;
        let read_ok = unsafe {
            InternetReadFile(
                request.0,
                chunk.as_mut_ptr() as *mut c_void,
                chunk.len() as u32,
                &mut bytes_read,
            )
        };
        if read_ok == 0 {
            return Err(wininet_error("reading the official feed body"));
        }
        if bytes_read == 0 {
            break;
        }
        if output.len().saturating_add(bytes_read as usize) > MAX_MINECRAFT_NEWS_BYTES {
            return Err("The official Minecraft News feed was larger than expected.".to_string());
        }
        output.extend_from_slice(&chunk[..bytes_read as usize]);
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

#[cfg(windows)]
fn fetch_official_minecraft_news() -> Result<Vec<MinecraftNewsArticle>, String> {
    let xml = fetch_minecraft_news_feed(MINECRAFT_NEWS_FEED_URL)?;
    parse_minecraft_news_feed(&xml)
}

#[cfg(not(windows))]
fn fetch_official_minecraft_news() -> Result<Vec<MinecraftNewsArticle>, String> {
    fetch_minecraft_news_from_url(MINECRAFT_NEWS_FEED_URL)
}

#[cfg(any(not(windows), test))]
fn fetch_minecraft_news_from_url(url: &str) -> Result<Vec<MinecraftNewsArticle>, String> {
    let xml = fetch_minecraft_news_feed(url)?;
    parse_minecraft_news_feed(&xml)
}

#[tauri::command]
fn fetch_minecraft_news(app: AppHandle) -> Result<MinecraftNewsResponse, String> {
    let cache_path = minecraft_news_cache_path(&app);
    match fetch_official_minecraft_news() {
        Ok(articles) => {
            let fetched_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let cache = CachedMinecraftNews {
                articles: articles.clone(),
                fetched_at,
            };
            if let Some(path) = cache_path.as_deref() {
                let _ = save_minecraft_news_cache(path, &cache);
            }
            Ok(MinecraftNewsResponse {
                articles,
                cached: false,
                fetched_at,
                error: None,
            })
        }
        Err(error) => {
            if let Some(cache) = cache_path.as_deref().and_then(load_minecraft_news_cache) {
                return Ok(MinecraftNewsResponse {
                    articles: cache.articles,
                    cached: true,
                    fetched_at: cache.fetched_at,
                    error: Some(error),
                });
            }
            Err(error)
        }
    }
}

#[tauri::command]
fn open_minecraft_news_link(url: String) -> Result<(), String> {
    if official_article_url(&url).as_deref() != Some(url.as_str()) {
        return Err(
            "Only links from the official Minecraft website can be opened here.".to_string(),
        );
    }
    #[cfg(windows)]
    let mut command = Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    command
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open the official Minecraft article: {error}"))
}

fn world_backup_directory(app: &AppHandle) -> Result<PathBuf, String> {
    config_file(app)?
        .parent()
        .map(|parent| parent.join("world-backups"))
        .ok_or_else(|| "Could not find the Bee Launcher data folder.".to_string())
}

fn world_backup_source(
    app: &AppHandle,
    profile_key: Option<&str>,
) -> Result<(PathBuf, String), String> {
    if let Some(key) = profile_key.filter(|key| !key.trim().is_empty()) {
        let (game_dir, _) = modpack::resolve_profile(app, key)?;
        return Ok((game_dir, key.to_string()));
    }
    Ok((
        resolve_game_dir(app)?,
        "Default Minecraft folder".to_string(),
    ))
}

#[tauri::command]
fn list_world_backups(app: AppHandle) -> Result<Vec<world_backup::BackupSummary>, String> {
    world_backup::list_backups(&world_backup_directory(&app)?)
}

#[tauri::command]
fn create_world_backup(
    app: AppHandle,
    profile_key: Option<String>,
) -> Result<world_backup::BackupSummary, String> {
    let (game_dir, label) = world_backup_source(&app, profile_key.as_deref())?;
    world_backup::create_backup(
        &world_backup_directory(&app)?,
        &game_dir.join("saves"),
        &label,
    )
}

#[tauri::command]
fn inspect_world_backup(
    app: AppHandle,
    backup_id: String,
    profile_key: Option<String>,
) -> Result<world_backup::RestorePreview, String> {
    let (game_dir, _) = world_backup_source(&app, profile_key.as_deref())?;
    world_backup::inspect_backup(
        &world_backup_directory(&app)?,
        &backup_id,
        &game_dir.join("saves"),
    )
}

#[tauri::command]
fn restore_world_backup(
    app: AppHandle,
    backup_id: String,
    profile_key: Option<String>,
    selected_worlds: Vec<String>,
) -> Result<world_backup::RestoreResult, String> {
    let (game_dir, _) = world_backup_source(&app, profile_key.as_deref())?;
    world_backup::restore_backup(
        &world_backup_directory(&app)?,
        &backup_id,
        &game_dir.join("saves"),
        &selected_worlds,
    )
}

#[tauri::command]
fn diagnose_launch_log(
    app: AppHandle,
    profile_key: String,
) -> Result<crash_helper::CrashDiagnosis, String> {
    let (game_dir, _) = modpack::resolve_profile(&app, &profile_key)?;
    Ok(crash_helper::analyze_log(
        &game_dir.join("logs").join("blockline-latest.log"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    #[ignore = "requires direct live access to minecraft.net"]
    fn fetches_official_minecraft_news_over_the_live_rust_client() {
        let articles = fetch_official_minecraft_news().expect("official News fetch should work");
        assert!(
            !articles.is_empty(),
            "official Minecraft RSS feed should contain articles"
        );
        assert!(articles.iter().all(|article| {
            official_article_url(&article.url).as_deref() == Some(article.url.as_str())
        }));
    }

    #[test]
    fn fetches_and_parses_official_rss_articles_over_the_rust_http_path() {
        use std::net::TcpListener;

        let body = concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0"><channel><title>Minecraft</title>"#,
            r#"<item><title>Marketplace Spotlight: The Sound Effects Edition</title><link>https://www.minecraft.net/en-us/article/marketplace-spotlight-sound-effects</link><description><![CDATA[<p>Discover seven new sound effect packs on Minecraft Marketplace, from retro arcade beats to spooky soundscapes and relaxing audio for your adventures.</p>]]></description></item>"#,
            r#"<item><title>New watch time challenge</title><link>https://www.minecraft.net/en-us/article/twitchcon-watch-time-quest</link><description><![CDATA[<p>Join the fun and earn a free add-on!</p>]]></description></item>"#,
            r#"<item><title>Untrusted item</title><link>https://example.invalid/fake</link><description>Must be discarded</description></item>"#,
            r#"</channel></rss>"#,
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/rss+xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let articles = fetch_minecraft_news_from_url(&format!("http://{address}/rss"))
            .expect("Rust news transport should fetch and parse the RSS response");
        server.join().unwrap();
        assert_eq!(articles.len(), 2);
        assert_eq!(
            articles[0].title,
            "Marketplace Spotlight: The Sound Effects Edition"
        );
        assert!(articles[0].summary.contains("seven new sound effect packs"));
        assert_eq!(
            articles[0].url,
            "https://www.minecraft.net/en-us/article/marketplace-spotlight-sound-effects"
        );
        assert_eq!(articles[1].title, "New watch time challenge");
        assert_eq!(articles[1].summary, "Join the fun and earn a free add-on!");
    }

    #[test]
    fn news_cache_roundtrips_and_rejects_non_minecraft_urls() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!("minecraft-news-cache-test-{unique}"));
        let path = directory.join("minecraft-news.json");
        let cache = CachedMinecraftNews {
            articles: vec![MinecraftNewsArticle {
                title: "Official article".to_string(),
                summary: "Cached summary".to_string(),
                url: "https://www.minecraft.net/en-us/article/example".to_string(),
            }],
            fetched_at: 1_800_000_000,
        };

        save_minecraft_news_cache(&path, &cache).unwrap();
        let loaded = load_minecraft_news_cache(&path).unwrap();
        assert_eq!(loaded.fetched_at, cache.fetched_at);
        assert_eq!(loaded.articles[0].url, cache.articles[0].url);

        let tampered = CachedMinecraftNews {
            articles: vec![MinecraftNewsArticle {
                title: "Untrusted article".to_string(),
                summary: String::new(),
                url: "https://example.invalid/article".to_string(),
            }],
            fetched_at: cache.fetched_at,
        };
        fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
        assert!(load_minecraft_news_cache(&path).is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    fn seed_profile_libraries_for_host(game_dir: &Path, chain: &[(String, Value)]) {
        use std::io::Write;

        for (_, metadata) in chain {
            let Some(libraries) = metadata.get("libraries").and_then(Value::as_array) else {
                continue;
            };
            for library in libraries {
                if !allowed_by_rules(library, current_os_name()) {
                    continue;
                }
                let name = library.get("name").and_then(Value::as_str).unwrap_or("");
                let is_native = name
                    .rsplit(':')
                    .next()
                    .is_some_and(|part| part.starts_with("natives-"));
                if is_native && !current_native_os_classifier(name) {
                    continue;
                }
                let Some(spec) = library_artifact_spec(library) else {
                    continue;
                };
                let path = safe_join(game_dir, &spec.relative_path).unwrap();
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                if is_native {
                    let file = fs::File::create(&path).unwrap();
                    let mut archive = zip::ZipWriter::new(file);
                    let extension = match current_os_name() {
                        "windows" => "dll",
                        "osx" => "dylib",
                        _ => "so",
                    };
                    archive
                        .start_file(
                            format!("libfixture.{extension}"),
                            zip::write::SimpleFileOptions::default(),
                        )
                        .unwrap();
                    archive.write_all(b"fixture native").unwrap();
                    archive.finish().unwrap();
                } else {
                    fs::write(path, b"fixture library jar").unwrap();
                }
            }
        }
    }

    #[test]
    fn prefers_populated_tlauncher_roaming_folder_and_finds_fabric_profile() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("blockline-scan-test-{unique}"));
        let local = root.join("LocalAppData/.minecraft");
        let roaming = root.join("AppData/Roaming/.minecraft");
        fs::create_dir_all(local.join("versions")).unwrap();
        fs::create_dir_all(roaming.join(".fabric")).unwrap();
        fs::create_dir_all(roaming.join("assets")).unwrap();
        fs::create_dir_all(roaming.join("libraries")).unwrap();
        fs::create_dir_all(roaming.join("runtime")).unwrap();
        fs::create_dir_all(roaming.join("mods")).unwrap();
        fs::write(roaming.join("TlauncherProfiles.json"), "{}").unwrap();
        fs::write(roaming.join("TLauncher.exe"), b"launcher marker").unwrap();
        fs::write(roaming.join("mods/example-mod.jar"), b"mod marker").unwrap();

        let fabric_id = "Fabric 26.3";
        let fabric_dir = roaming.join("versions").join(fabric_id);
        fs::create_dir_all(&fabric_dir).unwrap();
        fs::write(
            fabric_dir.join("Fabric 26.3.json"),
            r#"{"id":"Fabric 26.3","type":"release","mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient","libraries":[{"name":"net.fabricmc:fabric-loader:0.16.9"}] }"#,
        )
        .unwrap();
        fs::write(fabric_dir.join("Fabric 26.3.jar"), b"test jar").unwrap();

        let (local_score, _) = game_directory_score(&local);
        let (roaming_score, _) = game_directory_score(&roaming);
        assert!(roaming_score > local_score);
        let profiles = installed_versions(&roaming);
        let fabric = profiles
            .iter()
            .find(|profile| profile.id == fabric_id)
            .unwrap();
        assert_eq!(fabric.kind, "Fabric");
        assert!(fabric.launchable);

        let legacy_settings: StoredSettings = serde_json::from_str(
            r#"{"gameDirectory":"C:\\Users\\dillr\\AppData\\Local\\.minecraft"}"#,
        )
        .unwrap();
        assert!(!legacy_settings.game_directory_manual);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uploaded_fabric_26_3_profile_is_standalone_and_uses_its_own_jar() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("fabric-26-3-profile-test-{unique}"));
        let game_dir = root.join("Users/dillr/AppData/Roaming/.minecraft");
        let local = root.join("Users/dillr/AppData/Local/.minecraft");
        let fabric_id = "Fabric 26.3";
        let profile_dir = game_dir.join("versions").join(fabric_id);
        fs::create_dir_all(&profile_dir).unwrap();
        fs::create_dir_all(local.join("versions")).unwrap();
        fs::create_dir_all(game_dir.join("mods")).unwrap();
        fs::create_dir_all(profile_dir.join("natives")).unwrap();
        fs::write(
            profile_dir.join("Fabric 26.3.json"),
            include_str!("../tests/fixtures/Fabric26.3.json"),
        )
        .unwrap();
        fs::write(
            profile_dir.join("Fabric 26.3.jar"),
            b"test placeholder; the user's 39.6 MB client jar is not bundled",
        )
        .unwrap();
        fs::write(
            profile_dir.join("TLauncherAdditional.json"),
            r#"{"jar":"26.3"}"#,
        )
        .unwrap();
        fs::write(game_dir.join("mods/example-mod.jar"), b"mod marker").unwrap();

        let metadata = metadata_at(&game_dir, fabric_id).unwrap();
        assert_eq!(metadata.get("id").and_then(Value::as_str), Some(fabric_id));
        assert!(metadata.get("inheritsFrom").is_none());
        assert!(
            metadata.get("jar").is_none(),
            "sidecar keys must not replace profile metadata"
        );
        assert_eq!(
            metadata.pointer("/assetIndex/id").and_then(Value::as_str),
            Some("34")
        );
        assert_eq!(
            metadata
                .get("libraries")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            121
        );
        let lz4_library = metadata
            .get("libraries")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .find(|library| {
                library.get("name").and_then(Value::as_str) == Some("at.yawk.lz4:lz4-java:1.10.1")
            })
            .unwrap();
        let lz4 = library_artifact_spec(lz4_library).unwrap();
        assert_eq!(
            lz4.url,
            "https://libraries.minecraft.net/at/yawk/lz4/lz4-java/1.10.1/lz4-java-1.10.1.jar"
        );
        assert_eq!(lz4.size, Some(910_232));
        assert_eq!(
            lz4.sha1.as_deref(),
            Some("f541d7f910fe3d76f38f799c507c48cc81b12ecb")
        );
        assert_eq!(
            lz4.fallback_urls.first().map(String::as_str),
            Some("https://repo.maven.apache.org/maven2/at/yawk/lz4/lz4-java/1.10.1/lz4-java-1.10.1.jar")
        );
        let authlib_chain = vec![(fabric_id.to_string(), metadata.clone())];
        let authlib = bundled_authlib_spec(&authlib_chain).unwrap();
        assert_eq!(authlib.relative_path, BUNDLED_AUTHLIB_PATH);
        assert_eq!(authlib.size, Some(BUNDLED_AUTHLIB_SIZE));
        assert_eq!(authlib.sha1.as_deref(), Some(BUNDLED_AUTHLIB_SHA1));
        assert!(authlib.fallback_urls.is_empty());
        assert_eq!(
            metadata.get("mainClass").and_then(Value::as_str),
            Some("net.fabricmc.loader.impl.launch.knot.KnotClient")
        );

        let chain = metadata_chain(&game_dir, fabric_id).unwrap();
        seed_profile_libraries_for_host(&game_dir, &chain);
        assert_eq!(
            chain.len(),
            1,
            "a profile without inheritsFrom is standalone"
        );
        assert_eq!(chain[0].0, fabric_id);
        let client_jar_path = profile_dir.join("Fabric 26.3.jar");
        let client_jar_placeholder = fs::read(&client_jar_path).unwrap();
        fs::remove_file(&client_jar_path).unwrap();
        assert!(
            installed_versions(&game_dir)
                .iter()
                .find(|version| version.id == fabric_id)
                .unwrap()
                .launchable
        );
        fs::write(&client_jar_path, client_jar_placeholder).unwrap();
        assert_eq!(required_java_major(&metadata), Some(25));
        let client_download = metadata.pointer("/downloads/client").unwrap();
        let client_spec = artifact_spec(
            client_download,
            Some("versions/Fabric 26.3/Fabric 26.3.jar".to_string()),
            client_download
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
        .unwrap();
        assert_eq!(client_spec.size, Some(41_483_720));
        assert_eq!(
            client_spec.sha1.as_deref(),
            Some("e877b6a07acd633fb3bb475002175cec036e7b87")
        );
        assert_eq!(
            resolve_client_jar(&game_dir, fabric_id, &chain).unwrap(),
            profile_dir.join("Fabric 26.3.jar")
        );

        let default_jvm = argument_values(metadata.pointer("/arguments/default_user_jvm"), "linux");
        assert!(default_jvm.iter().any(|arg| arg == "-Xms2G"));
        let jvm = argument_values(metadata.pointer("/arguments/jvm"), "linux");
        assert!(jvm.iter().any(|arg| arg == "-cp"));
        assert!(jvm
            .iter()
            .any(|arg| arg == "-Djava.library.path=${natives_directory}/java"));
        let game = argument_values(metadata.pointer("/arguments/game"), "linux");
        assert!(game.iter().any(|arg| arg == "--gameDir"));
        assert!(game.iter().any(|arg| arg == "${game_directory}"));

        let (classpath, native_dir) =
            classpath_and_extract_natives(&game_dir, fabric_id, &chain).unwrap();
        assert_eq!(native_dir, profile_dir.join("natives"));
        assert!(classpath.contains("fabric-loader-0.19.5.jar"));
        assert!(classpath.contains("net/sf/jopt-simple/jopt-simple/5.0.4/jopt-simple-5.0.4.jar"));
        for subdirectory in ["java", "jna", "lwjgl", "netty"] {
            assert!(native_dir.join(subdirectory).is_dir());
        }
        assert!(native_files_exist(&native_dir.join("java")));

        let fabric_loader = metadata
            .get("libraries")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .find(|library| {
                library.get("name").and_then(Value::as_str)
                    == Some("net.fabricmc:fabric-loader:0.19.5")
            })
            .unwrap();
        let missing_spec = library_artifact_spec(fabric_loader).unwrap();
        let missing_path = safe_join(&game_dir, &missing_spec.relative_path).unwrap();
        fs::remove_file(&missing_path).unwrap();
        let missing_error =
            classpath_and_extract_natives(&game_dir, fabric_id, &chain).unwrap_err();
        assert!(missing_error.contains("required Minecraft library is missing"));
        assert!(game_dir.join("mods/example-mod.jar").is_file());

        let versions = installed_versions(&game_dir);
        let profile = versions
            .iter()
            .find(|version| version.id == fabric_id)
            .unwrap();
        assert_eq!(profile.kind, "Fabric");
        assert!(profile.launchable);
        let (local_score, _) = game_directory_score(&local);
        let (roaming_score, _) = game_directory_score(&game_dir);
        assert!(roaming_score > local_score);

        let mut misleading_jar_hint = metadata;
        misleading_jar_hint["jar"] = Value::String("26.3".to_string());
        fs::write(
            profile_dir.join("Fabric 26.3.json"),
            serde_json::to_vec(&misleading_jar_hint).unwrap(),
        )
        .unwrap();
        let chain = metadata_chain(&game_dir, fabric_id).unwrap();
        assert_eq!(chain.len(), 1, "a jar field is not inheritance");
        assert_eq!(
            resolve_client_jar(&game_dir, fabric_id, &chain).unwrap(),
            profile_dir.join("Fabric 26.3.jar")
        );

        assert!(version_in_range(
            "10.0.26100.1",
            &serde_json::json!({"min":"10.0.17134"})
        ));
        assert!(!version_in_range(
            "10.0.26100.1",
            &serde_json::json!({"max":"10.0.17134"})
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundled_authlib_matches_profile_and_installs_to_expected_library_path() {
        let metadata: Value =
            serde_json::from_str(include_str!("../tests/fixtures/Fabric26.3.json")).unwrap();
        let chain = vec![("Fabric 26.3".to_string(), metadata)];
        let spec = bundled_authlib_spec(&chain).unwrap();
        assert_eq!(spec.relative_path, BUNDLED_AUTHLIB_PATH);
        assert_eq!(spec.size, Some(BUNDLED_AUTHLIB_SIZE));
        assert_eq!(spec.sha1.as_deref(), Some(BUNDLED_AUTHLIB_SHA1));
        assert_eq!(BUNDLED_AUTHLIB_BYTES.len() as u64, BUNDLED_AUTHLIB_SIZE);
        assert_eq!(sha1_bytes(BUNDLED_AUTHLIB_BYTES), BUNDLED_AUTHLIB_SHA1);

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("blockline-authlib-test-{unique}"));
        let installed = install_bundled_authlib(&root, &chain).unwrap().unwrap();
        assert_eq!(
            installed,
            root.join("libraries/com/mojang/authlib/10.0.77/authlib-10.0.77.jar")
        );
        assert!(artifact_matches(
            &installed,
            Some(BUNDLED_AUTHLIB_SIZE),
            Some(BUNDLED_AUTHLIB_SHA1)
        ));
        assert!(zip::ZipArchive::new(fs::File::open(&installed).unwrap()).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn adds_profile_declared_jopt_simple_to_classpath_and_checks_its_class() {
        use std::io::Write;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("blockline-jopt-classpath-test-{unique}"));
        let game_dir = root.join(".minecraft");
        let metadata: Value =
            serde_json::from_str(include_str!("../tests/fixtures/Fabric26.3.json")).unwrap();
        let chain = vec![("Fabric 26.3".to_string(), metadata)];
        let spec = jopt_simple_spec(&chain).unwrap();
        let jar_path = safe_join(&game_dir, &spec.relative_path).unwrap();
        fs::create_dir_all(jar_path.parent().unwrap()).unwrap();
        let file = fs::File::create(&jar_path).unwrap();
        let mut jar = zip::ZipWriter::new(file);
        jar.start_file(
            "joptsimple/ValueConverter.class",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        jar.write_all(b"test class placeholder").unwrap();
        jar.finish().unwrap();

        let (classpath, resolved_path) =
            include_jopt_simple_classpath(&game_dir, &chain, "").unwrap();
        assert_eq!(resolved_path.as_deref(), Some(jar_path.as_path()));
        assert!(classpath.contains(&jar_path.to_string_lossy().to_string()));
        let (unchanged, _) = include_jopt_simple_classpath(&game_dir, &chain, &classpath).unwrap();
        assert_eq!(
            unchanged, classpath,
            "do not add duplicate classpath entries"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selects_only_native_archives_for_the_target_platform() {
        assert!(native_coordinate_matches(
            "org.lwjgl:lwjgl:3.4.3:natives-windows",
            "windows",
            "x86_64"
        ));
        assert!(!native_coordinate_matches(
            "org.lwjgl:lwjgl:3.4.3:natives-windows-arm64",
            "windows",
            "x86_64"
        ));
        assert!(native_coordinate_matches(
            "org.lwjgl:lwjgl:3.4.3:natives-windows-arm64",
            "windows",
            "aarch64"
        ));
    }

    #[test]
    fn parses_java_runtime_versions_and_reports_early_child_exit() {
        assert_eq!(
            java_major_from_output("openjdk version \"25.0.1\" 2026"),
            Some(25)
        );
        assert_eq!(
            java_major_from_output("java version \"1.8.0_402\""),
            Some(8)
        );

        #[cfg(unix)]
        {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = env::temp_dir().join(format!("blockline-launch-exit-test-{unique}"));
            fs::create_dir_all(&root).unwrap();
            let log_path = root.join("blockline-latest.log");
            fs::write(
                &log_path,
                "Error: UnsupportedClassVersionError: test failure\n",
            )
            .unwrap();
            let mut child = Command::new("sh")
                .args(["-c", "exit 17"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let error = fail_if_early_exit(&mut child, &log_path).unwrap_err();
            assert!(error.contains("exit code 17"));
            assert!(error.contains("UnsupportedClassVersionError"));
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(test)]
mod custom_skin_tests {
    use super::{
        ensure_custom_skin_loader_local_skin_first, installed_custom_skin_loader_version,
        local_skin_loader_fingerprint, local_skin_priority_ready, parse_custom_skin_loader_version,
        prioritize_local_skin_profile, read_skin_png, safe_skin_id,
        skin_loader_log_has_fingerprint,
    };
    use std::{
        fs,
        io::Write,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn write_png(path: &Path, width: u32, height: u32) {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let pixels = vec![0; (width * height * 4) as usize];
            writer.write_image_data(&pixels).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    fn test_skin_path(label: &str) -> std::path::PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("bee-skin-{timestamp}-{label}.png"))
    }

    #[test]
    fn accepts_standard_and_legacy_skin_pngs() {
        for height in [64, 32] {
            let path = test_skin_path(&format!("height-{height}"));
            write_png(&path, 64, height);
            assert!(read_skin_png(&path).is_ok());
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn rejects_wrong_dimensions_and_malformed_pngs() {
        let wrong_size = test_skin_path("wrong-size");
        write_png(&wrong_size, 64, 63);
        assert!(read_skin_png(&wrong_size).unwrap_err().contains("64×64"));
        fs::remove_file(wrong_size).unwrap();

        let malformed = test_skin_path("malformed");
        fs::write(&malformed, b"not a png").unwrap();
        assert!(read_skin_png(&malformed).is_err());
        fs::remove_file(malformed).unwrap();
    }

    #[test]
    fn custom_skin_ids_cannot_escape_the_library_folder() {
        assert!(safe_skin_id("skin-123-example.png"));
        assert!(!safe_skin_id("../skin-123-example.png"));
        assert!(!safe_skin_id("skin-123-example.jpg"));
    }

    #[test]
    fn local_skin_is_moved_first_without_discarding_other_config() {
        let mut config = serde_json::json!({
            "threadPoolSize": 6,
            "loadlist": [
                { "name": "Mojang", "type": "MojangAPI" },
                { "name": "LocalSkin", "type": "Legacy", "skin": "wrong/path.png", "cape": "custom-cape", "custom": "kept" },
                { "name": "Other", "type": "Legacy" }
            ]
        });
        assert!(prioritize_local_skin_profile(&mut config).unwrap());
        assert_eq!(config["threadPoolSize"], 6);
        assert_eq!(config["loadlist"][0]["name"], "LocalSkin");
        assert_eq!(config["loadlist"][0]["type"], "Legacy");
        assert_eq!(
            config["loadlist"][0]["skin"],
            "LocalSkin/skins/{USERNAME}.png"
        );
        assert_eq!(config["loadlist"][0]["cape"], "custom-cape");
        assert_eq!(config["loadlist"][0]["custom"], "kept");
        assert_eq!(config["loadlist"][1]["name"], "Mojang");
        assert_eq!(config["loadlist"][2]["name"], "Other");
    }

    #[test]
    fn local_skin_already_first_needs_no_config_change() {
        let mut config = serde_json::json!({
            "loadlist": [
                {
                    "name": "LocalSkin",
                    "type": "Legacy",
                    "skin": "LocalSkin/skins/{USERNAME}.png",
                    "model": "auto",
                    "checkPNG": false,
                    "cape": "LocalSkin/capes/{USERNAME}.png",
                    "elytra": "LocalSkin/elytras/{USERNAME}.png"
                },
                { "name": "Mojang", "type": "MojangAPI" }
            ]
        });
        let before = config.clone();
        assert!(!prioritize_local_skin_profile(&mut config).unwrap());
        assert_eq!(config, before);
    }

    #[test]
    fn config_repair_refuses_a_non_array_loadlist() {
        let mut config = serde_json::json!({ "loadlist": "not an array" });
        let before = config.clone();
        assert!(prioritize_local_skin_profile(&mut config).is_err());
        assert_eq!(config, before);
    }

    #[test]
    fn local_skin_log_fingerprint_matches_the_current_local_file_reference() {
        let root = test_skin_path("skin-log-fingerprint").with_extension("");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("Player.png");
        fs::write(&path, b"verified skin bytes").unwrap();
        let relative = "LocalSkin/skins/Player.png";
        let fingerprint = local_skin_loader_fingerprint(&path, relative).unwrap();
        let log = format!("Player's profile loaded. (SkinUrl: (LOCAL_LEGACY){fingerprint},{relative} , Model: auto)\n");
        assert!(skin_loader_log_has_fingerprint(
            &log,
            &fingerprint,
            relative
        ));
        assert!(!skin_loader_log_has_fingerprint(
            &log,
            "0000000000000000000000000000000000000000",
            relative
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn config_repair_backs_up_the_original_and_moves_localskin_first() {
        let root = test_skin_path("config-backup").with_extension("");
        let game_dir = root.join("game");
        let backup_dir = root.join("launcher-backups");
        let loader_dir = game_dir.join("CustomSkinLoader");
        fs::create_dir_all(&loader_dir).unwrap();
        let config_path = loader_dir.join("CustomSkinLoader.json");
        let original = br#"{"loadlist":[{"name":"Mojang","type":"MojangAPI"},{"name":"LocalSkin","type":"Legacy"}],"threadPoolSize":4}"#;
        fs::write(&config_path, original).unwrap();

        let update = ensure_custom_skin_loader_local_skin_first(&game_dir, &backup_dir).unwrap();
        assert!(update.restart_required);
        assert!(update.backup_created);
        let backup = fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .next()
            .unwrap();
        assert_eq!(fs::read(backup).unwrap(), original);
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(config_path).unwrap()).unwrap();
        assert_eq!(config["loadlist"][0]["name"], "LocalSkin");
        assert_eq!(config["loadlist"][1]["name"], "Mojang");
        assert_eq!(config["threadPoolSize"], 4);
        assert!(local_skin_priority_ready(&game_dir).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_custom_skin_loader_config_stages_a_first_run_extra_list() {
        let root = test_skin_path("config-missing").with_extension("");
        let game_dir = root.join("game");
        let backup_dir = root.join("launcher-backups");
        fs::create_dir_all(&game_dir).unwrap();

        let update = ensure_custom_skin_loader_local_skin_first(&game_dir, &backup_dir).unwrap();
        assert!(update.restart_required);
        assert!(!update.backup_created);
        assert!(!backup_dir.exists());
        let extra_dir = game_dir.join("CustomSkinLoader/ExtraList");
        let helper = fs::read_dir(&extra_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .next()
            .unwrap();
        let profile: serde_json::Value =
            serde_json::from_slice(&fs::read(helper).unwrap()).unwrap();
        assert_eq!(profile["name"], "BeeLauncherLocalSkin");
        assert_eq!(profile["type"], "Legacy");
        assert_eq!(profile["skin"], "LocalSkin/skins/{USERNAME}.png");
        assert!(local_skin_priority_ready(&game_dir).unwrap());
        let repeated = ensure_custom_skin_loader_local_skin_first(&game_dir, &backup_dir).unwrap();
        assert!(repeated.restart_required);
        assert_eq!(fs::read_dir(extra_dir).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_official_custom_skin_loader_bootstrap_metadata_and_version() {
        let root = test_skin_path("mods-root").with_extension("");
        let mods = root.join("mods");
        fs::create_dir_all(&mods).unwrap();
        let jar = fs::File::create(mods.join("CustomSkinLoader_Universal-15.1.jar")).unwrap();
        let mut archive = zip::ZipWriter::new(jar);
        archive
            .start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(br#"{"schemaVersion":1,"id":"customskinloader-bootstrap","version":"15.1"}"#)
            .unwrap();
        archive.finish().unwrap();

        assert_eq!(
            installed_custom_skin_loader_version(&root).as_deref(),
            Some("15.1")
        );
        assert!(parse_custom_skin_loader_version("15.1") >= Some((15, 1, 0)));
        assert!(parse_custom_skin_loader_version("15.0.1") < Some((15, 1, 0)));
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(windows)]
fn set_windows_app_user_model_id() {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    // Must match the AppUserModelID written to the installer shortcuts and any
    // already-pinned shortcut that targets this executable.
    let app_id = WINDOWS_APP_USER_MODEL_ID
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr());
    }
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn set_windows_taskbar_icon(window: &tauri::WebviewWindow) {
    use windows_sys::Win32::{
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            LoadImageW, SendMessageW, SetClassLongPtrW, GCLP_HICON, GCLP_HICONSM, ICON_BIG,
            ICON_SMALL, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, WM_SETICON,
        },
    };

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let hwnd = hwnd.0 as windows_sys::Win32::Foundation::HWND;
    if hwnd.is_null() {
        return;
    }

    // Tauri embeds the configured ICO as Win32 resource 32512 in the executable.
    // Load that resource rather than relying on an installer shortcut or a file path.
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    let icon = unsafe {
        LoadImageW(
            module,
            32512usize as *const u16,
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE | LR_SHARED,
        )
    };
    if icon.is_null() {
        return;
    }

    let icon = icon as isize;
    unsafe {
        // WM_SETICON serves the live HWND; class icons cover the shell/taskbar fallback path.
        let _ = SetClassLongPtrW(hwnd, GCLP_HICON, icon);
        let _ = SetClassLongPtrW(hwnd, GCLP_HICONSM, icon);
        let _ = SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon);
        let _ = SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    set_windows_app_user_model_id();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let window = app
                .get_webview_window("main")
                .expect("Bee Launcher main window is configured");
            let icon = app
                .default_window_icon()
                .expect("Bee Launcher app icon is configured")
                .clone();
            window.set_icon(icon)?;
            #[cfg(all(windows, target_pointer_width = "64"))]
            set_windows_taskbar_icon(&window);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_status,
            get_launcher_settings,
            save_launcher_settings,
            save_profile_launch_settings,
            set_game_directory,
            set_wallpaper,
            get_wallpaper_data,
            read_local_image,
            list_custom_skins,
            import_custom_skin,
            set_active_custom_skin,
            remove_custom_skin,
            get_custom_skin_support,
            apply_custom_skin_to_profile,
            verify_custom_skin_application,
            reset_wallpaper,
            import_modpack,
            launch_version,
            fetch_minecraft_news,
            open_minecraft_news_link,
            list_world_backups,
            create_world_backup,
            inspect_world_backup,
            restore_world_backup,
            diagnose_launch_log,
            open_game_directory
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bee Launcher");
}
