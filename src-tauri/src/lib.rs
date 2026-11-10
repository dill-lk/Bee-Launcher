use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{
    collections::HashSet,
    env, fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

mod modpack;

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchResult {
    pid: Option<u32>,
    version_id: String,
    log_path: String,
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

fn launch_game(
    app: &AppHandle,
    game_dir: &Path,
    version_id: &str,
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
    let java_runtime = find_java_for_profile(game_dir, java_profile).ok_or_else(|| {
        required_major.map_or_else(
            || "A usable Java runtime was not found for this profile.".to_string(),
            |major| format!("This profile requires Java {major}; no compatible runtime was found."),
        )
    })?;
    let java = java_for_game_launch(&java_runtime);
    let http = downloader()?;
    ensure_client_jar(&http, game_dir, &chain)?;
    let bundled_authlib_path = install_bundled_authlib(game_dir, &chain)?;
    ensure_library_artifacts(&http, game_dir, &chain)?;
    if let Some(spec) = jopt_simple_spec(&chain) {
        ensure_downloaded(&http, game_dir, &spec, "jopt-simple support library")?;
    }
    let launch_settings = read_stored_settings(app);
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
    jvm_args.push(format!("-Xmx{}G", launch_settings.max_memory_gb));
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
    launch_game(&app, &game_dir, &version_id, &username)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

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
            set_game_directory,
            set_wallpaper,
            get_wallpaper_data,
            read_local_image,
            reset_wallpaper,
            import_modpack,
            launch_version,
            open_game_directory
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bee Launcher");
}
