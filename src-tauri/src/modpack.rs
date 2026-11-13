use super::*;
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::OpenOptions,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};
use tauri::Emitter;
use zip::ZipArchive;

const MAX_PACK_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_OVERRIDE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_PACK_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_TOTAL_PACK_FILE_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const MAX_PACK_FILES: usize = 50_000;
const MAX_ASSET_INDEX_BYTES: u64 = 64 * 1024 * 1024;
const MINECRAFT_VERSION_MANIFEST_URLS: [&str; 2] = [
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json",
];
const METADATA_REQUEST_ATTEMPTS: usize = 3;
static SHARED_GAME_FILES_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModrinthManifest {
    format_version: u32,
    game: String,
    #[serde(default)]
    version_id: Option<String>,
    name: String,
    files: Vec<ModrinthFile>,
    dependencies: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseForgeManifest {
    manifest_type: String,
    manifest_version: u32,
    name: String,
    version: String,
    minecraft: CurseForgeMinecraft,
    files: Vec<CurseForgeDependency>,
    #[serde(default)]
    overrides: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseForgeMinecraft {
    version: String,
    #[serde(default)]
    mod_loaders: Vec<CurseForgeLoader>,
    #[serde(default)]
    recommended_ram: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CurseForgeLoader {
    id: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseForgeDependency {
    #[serde(rename = "projectID")]
    project_id: u64,
    #[serde(rename = "fileID")]
    file_id: u64,
    #[serde(default = "default_required")]
    required: bool,
}

fn default_required() -> bool {
    true
}

#[derive(Debug)]
enum PackManifest {
    Modrinth(ModrinthManifest),
    CurseForge(CurseForgeManifest),
}

#[derive(Debug)]
struct PackDetails {
    name: String,
    version: Option<String>,
    minecraft_version: String,
    loader: Option<LoaderPin>,
    recommended_ram_mb: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CurseForgeFileResponse {
    data: CurseForgeFileMetadata,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseForgeFileMetadata {
    id: u64,
    project_id: u64,
    file_name: String,
    file_length: u64,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    is_compatible_with_client: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CurseForgeDestination {
    Mods,
    Resourcepacks,
    Shaderpacks,
}

#[derive(Clone, Debug)]
struct ResolvedCurseForgeFile {
    project_id: u64,
    file_id: u64,
    file_name: String,
    file_length: u64,
    download_url: String,
    relative_path: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModrinthFile {
    path: String,
    hashes: ModrinthHashes,
    #[serde(default)]
    env: Option<ModrinthEnvironment>,
    downloads: Vec<String>,
    file_size: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct ModrinthHashes {
    sha1: String,
    #[serde(default)]
    sha512: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ModrinthEnvironment {
    #[serde(default)]
    client: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoaderKind {
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl LoaderKind {
    fn dependency_key(self) -> &'static str {
        match self {
            Self::Fabric => "fabric-loader",
            Self::Quilt => "quilt-loader",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LoaderPin {
    kind: LoaderKind,
    version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstanceMarker {
    instance_id: String,
    name: String,
    minecraft_version: String,
    #[serde(default)]
    loader_name: Option<String>,
    #[serde(default)]
    loader_version: Option<String>,
    // Retained so markers created by older releases remain readable.
    #[serde(default)]
    fabric_loader: Option<String>,
    profile_id: String,
    pack_version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImportResult {
    instance_name: String,
    profile_key: String,
    minecraft_version: String,
    loader_name: Option<String>,
    loader_version: Option<String>,
    files_installed: usize,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportProgress {
    stage: String,
    message: String,
    current: usize,
    total: usize,
    percent: u8,
}

fn report_progress(
    app: &AppHandle,
    stage: &str,
    message: impl Into<String>,
    current: usize,
    total: usize,
    percent: u8,
) {
    let _ = app.emit(
        "modpack-import-progress",
        ImportProgress {
            stage: stage.to_string(),
            message: message.into(),
            current,
            total,
            percent: percent.min(100),
        },
    );
}

fn app_instances_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not find Bee Launcher's data folder: {error}"))?
        .join("instances");
    fs::create_dir_all(&root)
        .map_err(|error| format!("Could not create Bee Launcher's instances folder: {error}"))?;
    Ok(root)
}

fn app_shared_game_files_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not find Bee Launcher's data folder: {error}"))?
        .join("minecraft-common");
    fs::create_dir_all(&root)
        .map_err(|error| format!("Could not create the shared Minecraft file folder: {error}"))?;
    Ok(root)
}

fn files_have_same_bytes(left: &Path, right: &Path) -> io::Result<bool> {
    let left_meta = fs::metadata(left)?;
    let right_meta = fs::metadata(right)?;
    if !left_meta.is_file() || !right_meta.is_file() || left_meta.len() != right_meta.len() {
        return Ok(false);
    }
    let mut left_file = fs::File::open(left)?;
    let mut right_file = fs::File::open(right)?;
    let mut left_buffer = [0_u8; 64 * 1024];
    let mut right_buffer = [0_u8; 64 * 1024];
    loop {
        let left_count = left_file.read(&mut left_buffer)?;
        let right_count = right_file.read(&mut right_buffer)?;
        if left_count != right_count || left_buffer[..left_count] != right_buffer[..right_count] {
            return Ok(false);
        }
        if left_count == 0 {
            return Ok(true);
        }
    }
}

fn merge_shared_tree(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    let source_meta = fs::symlink_metadata(source)
        .map_err(|error| format!("Could not inspect {}: {error}", source.display()))?;
    if source_meta.file_type().is_symlink() || !source_meta.is_dir() {
        return Err(format!(
            "Refusing to share a non-directory or linked game folder: {}",
            source.display()
        ));
    }
    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "Could not prepare shared folder {}: {error}",
            destination.display()
        )
    })?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("Could not read {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("Could not inspect a game file: {error}"))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|error| format!("Could not inspect {}: {error}", source_path.display()))?;
        if kind.is_symlink() {
            return Err(format!(
                "Refusing to share linked game data: {}",
                source_path.display()
            ));
        }
        if kind.is_dir() {
            merge_shared_tree(&source_path, &destination_path)?;
        } else if kind.is_file() {
            if destination_path.exists() {
                if !files_have_same_bytes(&source_path, &destination_path).map_err(|error| {
                    format!(
                        "Could not compare shared file {}: {error}",
                        destination_path.display()
                    )
                })? {
                    return Err(format!(
                        "A different file already exists at {}. The existing profile was left intact and its common game files were not replaced.",
                        destination_path.display()
                    ));
                }
            } else {
                fs::copy(&source_path, &destination_path).map_err(|error| {
                    format!(
                        "Could not add {} to the shared game-file store: {error}",
                        source_path.display()
                    )
                })?;
            }
        } else {
            return Err(format!(
                "Refusing to share a special game file: {}",
                source_path.display()
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SharedGameStoreSeedState {
    version: u32,
    sources: Vec<String>,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct SharedGameStoreSeedResult {
    already_seeded: bool,
    copied_files: usize,
    conflicts_left_unchanged: usize,
}

fn copy_tree_into_shared_store(
    source: &Path,
    destination: &Path,
    result: &mut SharedGameStoreSeedResult,
) -> Result<(), String> {
    let source_meta = fs::symlink_metadata(source)
        .map_err(|error| format!("Could not inspect {}: {error}", source.display()))?;
    if source_meta.file_type().is_symlink() || !source_meta.is_dir() {
        return Err(format!(
            "Refusing to copy a linked or non-directory Minecraft folder: {}",
            source.display()
        ));
    }
    match fs::symlink_metadata(destination) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(format!(
                "Refusing to replace a linked or non-directory shared folder: {}",
                destination.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(destination).map_err(|error| {
                format!(
                    "Could not create shared Minecraft folder {}: {error}",
                    destination.display()
                )
            })?;
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect shared Minecraft folder {}: {error}",
                destination.display()
            ));
        }
    }

    for entry in fs::read_dir(source)
        .map_err(|error| format!("Could not read {}: {error}", source.display()))?
    {
        let entry =
            entry.map_err(|error| format!("Could not inspect a Minecraft file: {error}"))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let kind = fs::symlink_metadata(&source_path)
            .map_err(|error| format!("Could not inspect {}: {error}", source_path.display()))?
            .file_type();
        if kind.is_symlink() {
            return Err(format!(
                "Refusing to copy linked Minecraft data: {}",
                source_path.display()
            ));
        }
        if kind.is_dir() {
            copy_tree_into_shared_store(&source_path, &destination_path, result)?;
        } else if kind.is_file() {
            match fs::symlink_metadata(&destination_path) {
                Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
                    return Err(format!(
                        "Refusing to replace a linked or non-file shared path: {}",
                        destination_path.display()
                    ));
                }
                Ok(_) => {
                    if !files_have_same_bytes(&source_path, &destination_path).map_err(|error| {
                        format!(
                            "Could not compare shared Minecraft file {}: {error}",
                            destination_path.display()
                        )
                    })? {
                        result.conflicts_left_unchanged += 1;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::copy(&source_path, &destination_path).map_err(|error| {
                        format!(
                            "Could not physically copy {} into Bee's shared Minecraft store: {error}",
                            source_path.display()
                        )
                    })?;
                    result.copied_files += 1;
                }
                Err(error) => {
                    return Err(format!(
                        "Could not inspect shared Minecraft file {}: {error}",
                        destination_path.display()
                    ));
                }
            }
        } else {
            return Err(format!(
                "Refusing to copy a special Minecraft file: {}",
                source_path.display()
            ));
        }
    }
    Ok(())
}

fn seed_shared_game_store_from_local_base(
    source_root: &Path,
    common_root: &Path,
) -> Result<SharedGameStoreSeedResult, String> {
    let source_key = fs::canonicalize(source_root)
        .map_err(|error| format!("Could not resolve the selected .minecraft folder: {error}"))?
        .to_string_lossy()
        .into_owned();
    let state_path = common_root.join(".bee-local-base.json");
    let mut state = match fs::symlink_metadata(&state_path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            return Err("Bee's shared Minecraft seed marker is not a regular file.".to_string());
        }
        Ok(_) => {
            serde_json::from_slice::<SharedGameStoreSeedState>(&fs::read(&state_path).map_err(
                |error| format!("Could not read Bee's shared Minecraft seed marker: {error}"),
            )?)
            .map_err(|error| format!("Bee's shared Minecraft seed marker is invalid: {error}"))?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => SharedGameStoreSeedState {
            version: 1,
            sources: Vec::new(),
        },
        Err(error) => {
            return Err(format!(
                "Could not inspect Bee's shared Minecraft seed marker: {error}"
            ));
        }
    };
    if state.version != 1 {
        return Err("Bee's shared Minecraft seed marker uses an unsupported version.".to_string());
    }
    if state.sources.contains(&source_key) {
        return Ok(SharedGameStoreSeedResult {
            already_seeded: true,
            ..SharedGameStoreSeedResult::default()
        });
    }

    let mut available_folders = Vec::new();
    for folder in ["assets", "libraries", "versions"] {
        let source = source_root.join(folder);
        match fs::symlink_metadata(&source) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(format!(
                    "The selected .minecraft {folder} path is not a regular folder: {}",
                    source.display()
                ));
            }
            Ok(_) => available_folders.push((folder, source)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("Could not inspect {}: {error}", source.display()));
            }
        }
    }
    if available_folders.is_empty() {
        return Ok(SharedGameStoreSeedResult::default());
    }

    let common_root = fs::canonicalize(common_root)
        .map_err(|error| format!("Could not resolve Bee's shared Minecraft folder: {error}"))?;
    let mut result = SharedGameStoreSeedResult::default();
    for (folder, source) in available_folders {
        let canonical_source = fs::canonicalize(&source)
            .map_err(|error| format!("Could not resolve {}: {error}", source.display()))?;
        let destination = common_root.join(folder);
        if destination.starts_with(&canonical_source) {
            return Err(format!(
                "Bee's shared {folder} folder is inside the selected .minecraft source and cannot be safely copied."
            ));
        }
        copy_tree_into_shared_store(&source, &destination, &mut result)?;
    }

    state.sources.push(source_key);
    let marker = serde_json::to_vec_pretty(&state)
        .map_err(|error| format!("Could not encode Bee's shared Minecraft seed marker: {error}"))?;
    let temporary_marker = common_root.join(format!(".bee-local-base.{}.tmp", std::process::id()));
    fs::write(&temporary_marker, marker)
        .map_err(|error| format!("Could not write Bee's shared Minecraft seed marker: {error}"))?;
    let backup_marker = common_root.join(format!(".bee-local-base.{}.bak", std::process::id()));
    let had_previous_marker = state_path.exists();
    if had_previous_marker && backup_marker.exists() {
        let _ = fs::remove_file(&temporary_marker);
        return Err("A previous Bee Minecraft seed-marker backup already exists.".to_string());
    }
    if had_previous_marker {
        if let Err(error) = fs::rename(&state_path, &backup_marker) {
            let _ = fs::remove_file(&temporary_marker);
            return Err(format!(
                "Could not stage Bee's shared Minecraft seed marker: {error}"
            ));
        }
    }
    if let Err(error) = fs::rename(&temporary_marker, &state_path) {
        if had_previous_marker {
            let _ = fs::rename(&backup_marker, &state_path);
        }
        let _ = fs::remove_file(&temporary_marker);
        return Err(format!(
            "Could not finalize Bee's shared Minecraft seed marker: {error}"
        ));
    }
    if had_previous_marker {
        let _ = fs::remove_file(backup_marker);
    }
    Ok(result)
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    let output = std::process::Command::new("cmd.exe")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "Windows could not create a directory junction: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        ))
    }
}

#[cfg(not(any(unix, windows)))]
fn create_directory_link(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "directory links are not supported on this platform",
    ))
}

fn is_linked_to(path: &Path, target: &Path) -> bool {
    path.is_dir()
        && fs::canonicalize(path)
            .ok()
            .zip(fs::canonicalize(target).ok())
            .is_some_and(|(actual, expected)| actual == expected)
}

fn ensure_profile_shared_assets(profile_dir: &Path, common_dir: &Path) -> Result<(), String> {
    let shared_assets = common_dir.join("assets");
    match fs::symlink_metadata(&shared_assets) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(format!(
                "Refusing to use a linked or non-directory shared assets folder: {}",
                shared_assets.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&shared_assets)
                .map_err(|error| format!("Could not prepare shared Minecraft assets: {error}"))?;
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect shared Minecraft assets {}: {error}",
                shared_assets.display()
            ));
        }
    }

    let profile_assets = profile_dir.join("assets");
    if is_linked_to(&profile_assets, &shared_assets) {
        // Older profiles used a whole-directory junction. Leave it intact: new
        // imports no longer rewrite shared indexes, so these profiles stay safe.
        return Ok(());
    }
    match fs::symlink_metadata(&profile_assets) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(format!(
                "Refusing to replace a linked or non-directory profile assets path: {}",
                profile_assets.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&profile_assets).map_err(|error| {
                format!("Could not create the profile's local assets folder: {error}")
            })?;
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect profile assets {}: {error}",
                profile_assets.display()
            ));
        }
    }

    let local_indexes = profile_assets.join("indexes");
    match fs::symlink_metadata(&local_indexes) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(format!(
                "Refusing to use a linked or non-directory profile asset-index folder: {}",
                local_indexes.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&local_indexes).map_err(|error| {
                format!("Could not create the profile's local asset-index folder: {error}")
            })?;
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect profile asset indexes {}: {error}",
                local_indexes.display()
            ));
        }
    }

    let shared_objects = shared_assets.join("objects");
    match fs::symlink_metadata(&shared_objects) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(format!(
                "Refusing to use a linked or non-directory shared asset-object folder: {}",
                shared_objects.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&shared_objects).map_err(|error| {
                format!("Could not prepare shared Minecraft asset objects: {error}")
            })?;
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect shared asset objects {}: {error}",
                shared_objects.display()
            ));
        }
    }

    let profile_objects = profile_assets.join("objects");
    if is_linked_to(&profile_objects, &shared_objects) {
        return Ok(());
    }
    match fs::symlink_metadata(&profile_objects) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(format!(
                "Refusing to replace a linked or non-directory profile asset-object path: {}",
                profile_objects.display()
            ));
        }
        Ok(_) => merge_shared_tree(&profile_objects, &shared_objects)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Could not inspect profile asset objects {}: {error}",
                profile_objects.display()
            ));
        }
    }

    let backup = profile_assets.join(".objects.bee-local-backup");
    if backup.exists() {
        return Err(format!(
            "A previous safe asset-object migration copy already exists at {}; it was left untouched.",
            backup.display()
        ));
    }
    let had_local_dir = profile_objects.exists();
    if had_local_dir {
        fs::rename(&profile_objects, &backup).map_err(|error| {
            format!(
                "Could not stage {} for a safe shared asset-object switch: {error}",
                profile_objects.display()
            )
        })?;
    }
    if let Err(error) = create_directory_link(&shared_objects, &profile_objects) {
        if had_local_dir {
            let _ = fs::rename(&backup, &profile_objects);
        }
        return Err(format!(
            "Could not link this profile to shared asset objects: {error}"
        ));
    }
    if had_local_dir {
        if let Err(error) = fs::remove_dir_all(&backup) {
            return Err(format!(
                "The profile now uses shared asset objects, but its unchanged migration copy remains at {} because cleanup failed: {error}",
                backup.display()
            ));
        }
    }
    Ok(())
}

fn ensure_profile_shared_game_files(profile_dir: &Path, common_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(profile_dir)
        .map_err(|error| format!("Could not prepare the modpack profile folder: {error}"))?;
    ensure_profile_shared_assets(profile_dir, common_dir)?;
    for folder in ["libraries", "versions"] {
        let shared_path = common_dir.join(folder);
        fs::create_dir_all(&shared_path)
            .map_err(|error| format!("Could not prepare shared {folder}: {error}"))?;
        let profile_path = profile_dir.join(folder);
        if is_linked_to(&profile_path, &shared_path) {
            continue;
        }
        let metadata = fs::symlink_metadata(&profile_path);
        match metadata {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(format!(
                    "Refusing to replace a linked or non-directory profile path: {}",
                    profile_path.display()
                ));
            }
            Ok(_) => merge_shared_tree(&profile_path, &shared_path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Could not inspect {}: {error}",
                    profile_path.display()
                ))
            }
        }
        let backup = profile_dir.join(format!(".{folder}.bee-local-backup"));
        if backup.exists() {
            return Err(format!(
                "A previous safe migration copy already exists at {}; it was left untouched.",
                backup.display()
            ));
        }
        let had_local_dir = profile_path.exists();
        if had_local_dir {
            fs::rename(&profile_path, &backup).map_err(|error| {
                format!(
                    "Could not stage {} for a safe shared-store switch: {error}",
                    profile_path.display()
                )
            })?;
        }
        if let Err(error) = create_directory_link(&shared_path, &profile_path) {
            if had_local_dir {
                let _ = fs::rename(&backup, &profile_path);
            }
            return Err(format!(
                "Could not link this profile to shared {folder}: {error}"
            ));
        }
        if had_local_dir {
            if let Err(error) = fs::remove_dir_all(&backup) {
                return Err(format!(
                    "The profile now uses shared {folder}, but its unchanged migration copy remains at {} because cleanup failed: {error}",
                    backup.display()
                ));
            }
        }
    }
    Ok(())
}

fn safe_pack_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains(':')
        || path.contains('\0')
    {
        return Err(format!("The modpack contains an unsafe file path: {path}"));
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("The modpack contains an unsafe file path: {path}"));
    }
    let relative = PathBuf::from(path);
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("The modpack contains an unsafe file path: {path}"));
    }
    Ok(relative)
}

fn valid_sha1(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_sha512(value: &str) -> bool {
    value.len() == 128 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_dependency_version(value: &str, name: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
    {
        return Err(format!("The modpack's pinned {name} version is invalid."));
    }
    Ok(())
}

fn validate_manifest(manifest: &ModrinthManifest) -> Result<(String, Option<LoaderPin>), String> {
    if manifest.format_version != 1 {
        return Err(format!(
            "This Modrinth pack uses format version {}; Bee Launcher supports format version 1.",
            manifest.format_version
        ));
    }
    if !manifest.game.eq_ignore_ascii_case("minecraft") {
        return Err("This .mrpack is not a Minecraft modpack.".to_string());
    }
    if manifest.name.trim().is_empty() || manifest.name.trim().len() > 160 {
        return Err("The modpack has a missing or invalid display name.".to_string());
    }
    let minecraft = manifest
        .dependencies
        .get("minecraft")
        .ok_or_else(|| "The modpack does not pin a Minecraft version.".to_string())?;
    validate_dependency_version(minecraft, "Minecraft")?;
    let supported_loaders = [
        ("fabric-loader", LoaderKind::Fabric),
        ("quilt-loader", LoaderKind::Quilt),
        ("forge", LoaderKind::Forge),
        ("neoforge", LoaderKind::NeoForge),
    ];
    let selected: Vec<(&str, LoaderKind, &String)> = supported_loaders
        .iter()
        .filter_map(|(key, kind)| {
            manifest
                .dependencies
                .get(*key)
                .map(|version| (*key, *kind, version))
        })
        .collect();
    if selected.len() > 1 {
        return Err("This modpack pins more than one mod loader; Bee Launcher can import only one loader per profile.".to_string());
    }
    for dependency in manifest.dependencies.keys() {
        if dependency != "minecraft"
            && !supported_loaders
                .iter()
                .any(|(supported, _)| dependency == supported)
        {
            return Err(format!(
                "This pack pins the unsupported {dependency} dependency; Bee Launcher supports vanilla, Fabric, Quilt, Forge, and NeoForge packs."
            ));
        }
    }
    let loader = selected
        .first()
        .map(|(_, kind, version)| {
            validate_dependency_version(version, kind.label())?;
            Ok::<LoaderPin, String>(LoaderPin {
                kind: *kind,
                version: (*version).clone(),
            })
        })
        .transpose()?;
    if manifest.files.len() > MAX_PACK_FILES {
        return Err("This modpack contains too many files to import safely.".to_string());
    }
    let mut paths = HashSet::new();
    let mut total_bytes = 0_u64;
    for file in &manifest.files {
        safe_pack_path(&file.path)?;
        if !paths.insert(file.path.to_ascii_lowercase()) {
            return Err(format!(
                "The modpack lists the same file more than once: {}",
                file.path
            ));
        }
        if file.file_size == 0 || file.file_size > MAX_PACK_FILE_BYTES {
            return Err(format!(
                "The declared file size is invalid for {}.",
                file.path
            ));
        }
        total_bytes = total_bytes.saturating_add(file.file_size);
        if total_bytes > MAX_TOTAL_PACK_FILE_BYTES {
            return Err("The modpack's declared downloads are larger than Bee Launcher will import in one pass.".to_string());
        }
        if !valid_sha1(&file.hashes.sha1) {
            return Err(format!(
                "The modpack has an invalid SHA-1 checksum for {}.",
                file.path
            ));
        }
        if file
            .hashes
            .sha512
            .as_deref()
            .is_some_and(|hash| !valid_sha512(hash))
        {
            return Err(format!(
                "The modpack has an invalid SHA-512 checksum for {}.",
                file.path
            ));
        }
        if file.downloads.is_empty()
            || file
                .downloads
                .iter()
                .any(|url| !url.starts_with("https://"))
        {
            return Err(format!(
                "The modpack has no safe HTTPS download source for {}.",
                file.path
            ));
        }
    }
    Ok((minecraft.clone(), loader))
}

fn parse_curseforge_loader(id: &str) -> Result<LoaderPin, String> {
    let variants = [
        ("fabric-", LoaderKind::Fabric),
        ("quilt-loader-", LoaderKind::Quilt),
        ("quilt-", LoaderKind::Quilt),
        ("forge-", LoaderKind::Forge),
        ("neoforge-", LoaderKind::NeoForge),
    ];
    for (prefix, kind) in variants {
        if let Some(version) = id.strip_prefix(prefix) {
            validate_dependency_version(version, kind.label())?;
            return Ok(LoaderPin {
                kind,
                version: version.to_string(),
            });
        }
    }
    Err(format!(
        "This CurseForge pack pins the unsupported loader '{id}'. Bee Launcher supports Fabric, Quilt, Forge, and NeoForge."
    ))
}

fn validate_curseforge_manifest(manifest: &CurseForgeManifest) -> Result<PackDetails, String> {
    if manifest.manifest_type != "minecraftModpack" || manifest.manifest_version != 1 {
        return Err(
            "Bee Launcher supports CurseForge Minecraft modpack manifest version 1 only."
                .to_string(),
        );
    }
    if manifest.name.trim().is_empty() || manifest.name.trim().len() > 160 {
        return Err("The CurseForge pack has a missing or invalid display name.".to_string());
    }
    if manifest.version.trim().is_empty() || manifest.version.trim().len() > 100 {
        return Err("The CurseForge pack has a missing or invalid version.".to_string());
    }
    if manifest
        .overrides
        .as_deref()
        .is_some_and(|folder| folder != "overrides" && folder != "client-overrides")
    {
        return Err("This CurseForge ZIP uses an unsupported overrides folder name.".to_string());
    }
    validate_dependency_version(&manifest.minecraft.version, "Minecraft")?;
    if manifest.files.len() > MAX_PACK_FILES {
        return Err("This CurseForge pack contains too many files to import safely.".to_string());
    }
    let mut ids = HashSet::new();
    for file in &manifest.files {
        if !ids.insert((file.project_id, file.file_id)) {
            return Err(format!(
                "The CurseForge manifest lists project {} file {} more than once.",
                file.project_id, file.file_id
            ));
        }
    }
    let primary: Vec<&CurseForgeLoader> = manifest
        .minecraft
        .mod_loaders
        .iter()
        .filter(|loader| loader.primary)
        .collect();
    if primary.len() > 1 || manifest.minecraft.mod_loaders.len() > 1 {
        return Err("This CurseForge pack pins more than one loader; Bee Launcher can import only one loader per profile.".to_string());
    }
    let loader = manifest
        .minecraft
        .mod_loaders
        .first()
        .map(|loader| parse_curseforge_loader(&loader.id))
        .transpose()?;
    let recommended_ram_mb = manifest
        .minecraft
        .recommended_ram
        .filter(|ram| (1024..=131_072).contains(ram));
    Ok(PackDetails {
        name: manifest.name.clone(),
        version: Some(manifest.version.clone()),
        minecraft_version: manifest.minecraft.version.clone(),
        loader,
        recommended_ram_mb,
    })
}

fn read_manifest(path: &Path) -> Result<ModrinthManifest, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("Could not read the .mrpack file: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_PACK_ARCHIVE_BYTES {
        return Err("Choose a .mrpack file smaller than 256 MB.".to_string());
    }
    let file = fs::File::open(path)
        .map_err(|error| format!("Could not open the .mrpack file: {error}"))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("That .mrpack is not a valid ZIP archive: {error}"))?;
    let is_mrpack = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mrpack"));
    let manifests: Vec<String> = archive
        .file_names()
        .filter(|name| *name == "modrinth.index.json" || name.ends_with("/modrinth.index.json"))
        .map(str::to_string)
        .collect();
    if manifests.len() != 1
        || (is_mrpack
            && manifests
                .first()
                .is_some_and(|name| name != "modrinth.index.json"))
    {
        return Err("This archive must contain exactly one Modrinth index manifest; Bee bundle ZIPs may include it under their top-level folder.".to_string());
    }
    let manifest_name = manifests
        .first()
        .ok_or_else(|| "This archive has no Modrinth index manifest.".to_string())?;
    let mut manifest_file = archive
        .by_name(manifest_name)
        .map_err(|error| format!("Could not read the modpack manifest: {error}"))?;
    if manifest_file.size() > MAX_MANIFEST_BYTES {
        return Err("The modpack manifest is unexpectedly large.".to_string());
    }
    let mut bytes = Vec::with_capacity(manifest_file.size() as usize);
    manifest_file
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read the modpack manifest: {error}"))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("The modpack manifest is invalid: {error}"))
}

fn read_any_manifest(path: &Path) -> Result<PackManifest, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if !extension.eq_ignore_ascii_case("zip") {
        return read_manifest(path).map(PackManifest::Modrinth);
    }
    let file =
        fs::File::open(path).map_err(|error| format!("Could not open the ZIP modpack: {error}"))?;
    let archive = ZipArchive::new(file)
        .map_err(|error| format!("That modpack is not a valid ZIP archive: {error}"))?;
    let has_curseforge_manifest = archive.file_names().any(|name| name == "manifest.json");
    drop(archive);
    if has_curseforge_manifest {
        let file = fs::File::open(path)
            .map_err(|error| format!("Could not reopen the CurseForge ZIP: {error}"))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|error| format!("Could not read the CurseForge ZIP: {error}"))?;
        let mut entry = archive
            .by_name("manifest.json")
            .map_err(|error| format!("Could not read the CurseForge manifest: {error}"))?;
        if entry.size() > MAX_MANIFEST_BYTES {
            return Err("The CurseForge manifest is unexpectedly large.".to_string());
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Could not read the CurseForge manifest: {error}"))?;
        let manifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("The CurseForge manifest is invalid: {error}"))?;
        return Ok(PackManifest::CurseForge(manifest));
    }
    read_manifest(path).map(PackManifest::Modrinth)
}

fn safe_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "modpack".to_string()
    } else {
        slug
    }
}

fn allocate_instance_dir(instances: &Path, name: &str) -> Result<(String, PathBuf), String> {
    let base = safe_slug(name);
    for suffix in 1..=10_000_u32 {
        let instance_id = if suffix == 1 {
            base.clone()
        } else {
            format!("{base}-{suffix}")
        };
        let path = instances.join(&instance_id);
        match fs::create_dir(&path) {
            Ok(()) => return Ok((instance_id, path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Could not create the new modpack instance folder: {error}"
                ))
            }
        }
    }
    Err("Bee Launcher could not choose a unique folder for this modpack.".to_string())
}

fn copy_overrides(pack_path: &Path, destination: &Path, app: &AppHandle) -> Result<(), String> {
    let file = fs::File::open(pack_path)
        .map_err(|error| format!("Could not reopen the .mrpack file: {error}"))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("Could not read the .mrpack archive: {error}"))?;
    let mut copied = 0_usize;
    let mut total = 0_u64;
    let mut seen = HashSet::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not inspect a modpack archive entry: {error}"))?;
        let archive_name = entry.name().to_string();
        let Some(relative_name) = override_relative_path(&archive_name) else {
            continue;
        };
        if relative_name.is_empty() {
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!(
                "The modpack contains a symbolic-link override: {archive_name}"
            ));
        }
        let relative = safe_pack_path(relative_name)?;
        if relative
            .components()
            .next()
            .and_then(|component| component.as_os_str().to_str())
            .is_some_and(|root| {
                ["assets", "libraries", "versions", "runtime"]
                    .iter()
                    .any(|reserved| root.eq_ignore_ascii_case(reserved))
            })
        {
            return Err(format!(
                "The pack override {relative_name} targets Bee Launcher's shared game-file area and was refused."
            ));
        }
        let target = safe_join(destination, relative_name)?;
        if !seen.insert(relative.clone()) {
            return Err(format!(
                "The modpack contains duplicate override paths: {relative_name}"
            ));
        }
        if entry.is_dir() {
            fs::create_dir_all(&target)
                .map_err(|error| format!("Could not create an override folder: {error}"))?;
            continue;
        }
        if entry.size() > MAX_OVERRIDE_BYTES {
            return Err(format!(
                "An override file is too large to extract safely: {relative_name}"
            ));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_OVERRIDE_BYTES {
            return Err("The modpack's override files exceed the 2 GB safety limit.".to_string());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create a modpack override folder: {error}"))?;
        }
        let mut output = fs::File::create(&target).map_err(|error| {
            format!("Could not write modpack override {relative_name}: {error}")
        })?;
        let copied_bytes = io::copy(&mut entry, &mut output).map_err(|error| {
            format!("Could not extract modpack override {relative_name}: {error}")
        })?;
        if copied_bytes != entry.size() {
            return Err(format!(
                "The modpack override {relative_name} was truncated while extracting."
            ));
        }
        copied += 1;
    }
    report_progress(
        app,
        "overrides",
        if copied == 0 {
            "No override files in this pack"
        } else {
            "Modpack overrides copied to its isolated instance"
        },
        copied,
        copied,
        12,
    );
    Ok(())
}

fn override_relative_path(archive_name: &str) -> Option<&str> {
    for folder in ["overrides/", "client-overrides/"] {
        if let Some(index) = archive_name.rfind(folder) {
            if index == 0 || archive_name.as_bytes().get(index.wrapping_sub(1)) == Some(&b'/') {
                return Some(&archive_name[index + folder.len()..]);
            }
        }
    }
    None
}

fn request_json_once(
    client: &Client,
    url: &str,
    label: &str,
    expected_sha1: Option<&str>,
    limit: u64,
) -> Result<Value, String> {
    if !url.starts_with("https://") {
        return Err(format!("Refusing a non-HTTPS metadata source for {label}."));
    }
    let response = client
        .get(url)
        .timeout(Duration::from_secs(45))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("Could not fetch {label}: {error}"))?;
    if response.url().scheme() != "https" {
        return Err(format!("The {label} source redirected to a non-HTTPS URL."));
    }
    let mut bytes = Vec::new();
    response
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read {label}: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("The downloaded {label} is unexpectedly large."));
    }
    if let Some(expected) = expected_sha1 {
        if !sha1_bytes(&bytes).eq_ignore_ascii_case(expected) {
            return Err(format!(
                "The downloaded {label} failed its SHA-1 integrity check."
            ));
        }
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("The downloaded {label} is not valid JSON: {error}"))
}

fn request_json(
    client: &Client,
    url: &str,
    label: &str,
    expected_sha1: Option<&str>,
    limit: u64,
) -> Result<Value, String> {
    let mut last_error = None;
    for attempt in 0..METADATA_REQUEST_ATTEMPTS {
        match request_json_once(client, url, label, expected_sha1, limit) {
            Ok(value) => return Ok(value),
            Err(error) => last_error = Some(error),
        }
        if attempt + 1 < METADATA_REQUEST_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(if attempt == 0 { 300 } else { 900 }));
        }
    }
    Err(format!(
        "{} Retried {METADATA_REQUEST_ATTEMPTS} times; check your connection and try importing again.",
        last_error.unwrap_or_else(|| format!("Could not fetch {label}."))
    ))
}

fn request_json_from_sources(
    client: &Client,
    sources: &[String],
    label: &str,
    expected_sha1: Option<&str>,
    limit: u64,
) -> Result<Value, String> {
    let mut failures = Vec::new();
    for source in sources {
        match request_json(client, source, label, expected_sha1, limit) {
            Ok(value) => return Ok(value),
            Err(error) => failures.push(format!("{source}: {error}")),
        }
    }
    Err(format!(
        "Could not fetch {label} from Mojang's official metadata hosts. Check your connection and try importing again. {}",
        failures.join(" | ")
    ))
}

fn mojang_version_metadata_sources(url: &str) -> Result<Vec<String>, String> {
    let (alternate_host, path) =
        if let Some(path) = url.strip_prefix("https://piston-meta.mojang.com/") {
            ("launchermeta.mojang.com", path)
        } else if let Some(path) = url.strip_prefix("https://launchermeta.mojang.com/") {
            ("piston-meta.mojang.com", path)
        } else {
            return Err("Mojang's version list contains an untrusted metadata URL.".to_string());
        };
    Ok(vec![
        url.to_string(),
        format!("https://{alternate_host}/{path}"),
    ])
}

fn minecraft_version_manifest_sources() -> Vec<String> {
    MINECRAFT_VERSION_MANIFEST_URLS
        .iter()
        .map(|url| (*url).to_string())
        .collect()
}

fn minecraft_version_manifest(client: &Client, minecraft: &str) -> Result<Value, String> {
    let mut failures = Vec::new();
    for source in minecraft_version_manifest_sources() {
        match request_json(
            client,
            &source,
            "the Minecraft version list",
            None,
            16 * 1024 * 1024,
        ) {
            Ok(manifest) => {
                let versions = manifest.get("versions").and_then(Value::as_array);
                if versions.is_some_and(|versions| {
                    versions
                        .iter()
                        .any(|entry| entry.get("id").and_then(Value::as_str) == Some(minecraft))
                }) {
                    return Ok(manifest);
                }
                failures.push(format!(
                    "{source}: the response does not list Minecraft {minecraft}"
                ));
            }
            Err(error) => failures.push(format!("{source}: {error}")),
        }
    }
    Err(format!(
        "Could not fetch Minecraft {minecraft} from Mojang's official metadata hosts. Check your connection and try importing again. {}",
        failures.join(" | ")
    ))
}

fn write_json(path: &Path, value: &Value, label: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create the {label} folder: {error}"))?;
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("Could not encode {label}: {error}"))?;
    fs::write(path, bytes).map_err(|error| format!("Could not save {label}: {error}"))
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn version_is_a_profile_token(value: &str, version: &str) -> bool {
    value.match_indices(version).any(|(start, matched)| {
        let end = start + matched.len();
        let before_is_boundary = value[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '.');
        let after_is_boundary = value[end..]
            .chars()
            .next()
            .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '.');
        before_is_boundary && after_is_boundary
    })
}

fn loader_library_matches(name: &str, minecraft: &str, loader: &LoaderPin) -> bool {
    let Some((group_and_artifact, version)) = name.rsplit_once(':') else {
        return false;
    };
    match loader.kind {
        LoaderKind::Fabric => {
            group_and_artifact == "net.fabricmc:fabric-loader" && version == loader.version
        }
        LoaderKind::Quilt => {
            group_and_artifact == "org.quiltmc:quilt-loader" && version == loader.version
        }
        LoaderKind::Forge => {
            let expected = if loader.version.starts_with(&format!("{minecraft}-")) {
                loader.version.clone()
            } else {
                format!("{minecraft}-{}", loader.version)
            };
            group_and_artifact == "net.minecraftforge:forge" && version == expected
        }
        LoaderKind::NeoForge => {
            group_and_artifact == "net.neoforged:neoforge" && version == loader.version
        }
    }
}

fn metadata_has_loader(metadata: &Value, minecraft: &str, loader: &LoaderPin) -> bool {
    metadata
        .get("libraries")
        .and_then(Value::as_array)
        .is_some_and(|libraries| {
            libraries.iter().any(|library| {
                library
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| loader_library_matches(name, minecraft, loader))
            })
        })
}

fn profile_id_has_loader(profile_id: &str, loader: &LoaderPin) -> bool {
    let profile = profile_id.to_ascii_lowercase();
    let marker = match loader.kind {
        LoaderKind::Fabric => "fabric",
        LoaderKind::Quilt => "quilt",
        LoaderKind::Forge => "forge",
        LoaderKind::NeoForge => "neoforge",
    };
    profile.contains(marker) && version_is_a_profile_token(profile_id, &loader.version)
}

fn find_installed_pack_profile(
    game_dir: &Path,
    minecraft: &str,
    loader: Option<&LoaderPin>,
) -> Result<(String, Vec<(String, Value)>), String> {
    find_installed_pack_profile_with_jar(game_dir, minecraft, loader, true)
}

fn find_installed_loader_profile(
    game_dir: &Path,
    minecraft: &str,
    loader: &LoaderPin,
) -> Result<(String, Vec<(String, Value)>), String> {
    find_installed_pack_profile_with_jar(game_dir, minecraft, Some(loader), false)
}

fn find_installed_pack_profile_with_jar(
    game_dir: &Path,
    minecraft: &str,
    loader: Option<&LoaderPin>,
    require_client_jar: bool,
) -> Result<(String, Vec<(String, Value)>), String> {
    let versions_dir = game_dir.join("versions");
    let entries = fs::read_dir(&versions_dir).map_err(|error| {
        format!(
            "Could not find installed Minecraft profiles in {}: {error}",
            game_dir.display()
        )
    })?;
    let mut profile_dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .collect();
    profile_dirs.sort();

    for profile_dir in profile_dirs {
        let profile_id = profile_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if ensure_version_id(profile_id).is_err() {
            continue;
        }
        let Some(metadata) = super::metadata_at(game_dir, profile_id) else {
            continue;
        };
        if metadata.get("id").and_then(Value::as_str) != Some(profile_id) {
            continue;
        }
        let Ok(chain) = super::metadata_chain(game_dir, profile_id) else {
            continue;
        };
        let matches_minecraft =
            if let Some(parent) = metadata.get("inheritsFrom").and_then(Value::as_str) {
                chain.iter().any(|(id, _)| id == minecraft) || parent == minecraft
            } else if profile_id == minecraft {
                true
            } else {
                version_is_a_profile_token(profile_id, minecraft)
            };
        if !matches_minecraft {
            continue;
        }
        let matches_loader = match loader {
            Some(loader) => {
                chain.iter().any(|(id, item)| {
                    metadata_has_loader(item, minecraft, loader)
                        || profile_id_has_loader(id, loader)
                }) || profile_id_has_loader(profile_id, loader)
            }
            None => profile_id == minecraft,
        };
        if !matches_loader {
            continue;
        }
        let has_main_class = chain
            .iter()
            .any(|(_, item)| item.get("mainClass").and_then(Value::as_str).is_some());
        let has_client_jar = chain
            .iter()
            .any(|(id, _)| super::version_jar(game_dir, id).is_some());
        if !has_main_class || (require_client_jar && !has_client_jar) {
            continue;
        }
        return Ok((profile_id.to_string(), chain));
    }

    let loader = loader
        .map(|loader| format!(" with {} {}", loader.kind.label(), loader.version))
        .unwrap_or_default();
    Err(format!(
        "This ZIP needs Minecraft {minecraft}{loader}, but that profile is not installed in the selected .minecraft folder. Install and launch that profile once, then import the ZIP again. Bee ZIP imports do not fetch Minecraft's version list."
    ))
}

fn copy_local_file(
    source: &Path,
    destination: &Path,
    expected_size: Option<u64>,
    expected_sha1: Option<&str>,
    label: &str,
) -> Result<(), String> {
    if artifact_matches(destination, expected_size, expected_sha1) {
        return Ok(());
    }
    if destination.exists() {
        return Err(format!(
            "A different file already exists at {}; it was left unchanged instead of being replaced with {label}.",
            destination.display()
        ));
    }
    if !artifact_matches(source, expected_size, expected_sha1) {
        return Err(format!(
            "Could not reuse the installed {label} from {}. Open that Minecraft profile once so its files are installed, then retry the ZIP import.",
            source.display()
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| format!("Invalid destination for {label}."))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the folder for {label}: {error}"))?;
    fs::copy(source, destination).map_err(|error| {
        format!("Could not copy the installed {label} into Bee Launcher's shared store: {error}")
    })?;
    if !artifact_matches(destination, expected_size, expected_sha1) {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "The reused local {label} failed its integrity check."
        ));
    }
    Ok(())
}

fn copy_installed_profile(
    source_game_dir: &Path,
    destination_game_dir: &Path,
    chain: &[(String, Value)],
) -> Result<(), String> {
    for (id, metadata) in chain {
        ensure_version_id(id)?;
        let json_path = safe_join(destination_game_dir, &format!("versions/{id}/{id}.json"))?;
        if json_path.exists() {
            if super::metadata_at(destination_game_dir, id).as_ref() != Some(metadata) {
                return Err(format!(
                    "A different Minecraft profile metadata file already exists at {}; it was left unchanged.",
                    json_path.display()
                ));
            }
        } else {
            write_json(&json_path, metadata, "reused Minecraft profile metadata")?;
        }
        let source_jar = super::version_jar(source_game_dir, id).ok_or_else(|| {
            format!(
                "The installed profile {id} is missing its client JAR; launch it once, then retry."
            )
        })?;
        let destination_jar = safe_join(destination_game_dir, &format!("versions/{id}/{id}.jar"))?;
        let client = metadata.pointer("/downloads/client");
        let size = client
            .and_then(|item| item.get("size"))
            .and_then(Value::as_u64);
        let sha1 = client
            .and_then(|item| item.get("sha1"))
            .and_then(Value::as_str);
        copy_local_file(
            &source_jar,
            &destination_jar,
            size,
            sha1,
            &format!("Minecraft client JAR for {id}"),
        )?;
    }
    Ok(())
}

fn copy_installed_libraries(
    source_game_dir: &Path,
    destination_game_dir: &Path,
    chain: &[(String, Value)],
) -> Result<(), String> {
    let mut prepared = HashSet::new();
    for (_, metadata) in chain {
        let Some(libraries) = metadata.get("libraries").and_then(Value::as_array) else {
            continue;
        };
        for library in libraries {
            if !super::allowed_by_rules(library, super::current_os_name()) {
                continue;
            }
            let name = library
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Minecraft library");
            if name
                .rsplit(':')
                .next()
                .is_some_and(|part| part.starts_with("natives-"))
                && !super::current_native_os_classifier(name)
            {
                continue;
            }
            if let Some(spec) = library_artifact_spec(library) {
                if prepared.insert(spec.relative_path.clone()) {
                    copy_local_artifact(source_game_dir, destination_game_dir, &spec, name)?;
                }
            }
            if let Some((classifier, _)) = super::native_classifier(library) {
                if let Some(artifact) = library
                    .pointer("/downloads/classifiers")
                    .and_then(|classifiers| classifiers.get(&classifier))
                {
                    let mut spec = artifact_spec(artifact, None, None).ok_or_else(|| {
                        format!("Native library {name} has incomplete download metadata.")
                    })?;
                    spec.relative_path = super::minecraft_library_destination(&spec.relative_path);
                    if prepared.insert(spec.relative_path.clone()) {
                        copy_local_artifact(
                            source_game_dir,
                            destination_game_dir,
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

fn copy_local_artifact(
    source_game_dir: &Path,
    destination_game_dir: &Path,
    spec: &ArtifactSpec,
    label: &str,
) -> Result<(), String> {
    let source = safe_join(source_game_dir, &spec.relative_path)?;
    let destination = safe_join(destination_game_dir, &spec.relative_path)?;
    copy_local_file(
        &source,
        &destination,
        spec.size,
        spec.sha1.as_deref(),
        label,
    )
}

fn copy_installed_assets(
    app: Option<&AppHandle>,
    source_game_dir: &Path,
    destination_game_dir: &Path,
    profile_game_dir: &Path,
    chain: &[(String, Value)],
) -> Result<(), String> {
    let asset_index = chain
        .iter()
        .find_map(|(_, metadata)| metadata.get("assetIndex"))
        .ok_or_else(|| {
            "The installed Minecraft profile has no asset-index metadata.".to_string()
        })?;
    let index_id = asset_index
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "The Minecraft asset index has no ID.".to_string())?;
    if index_id.is_empty()
        || !index_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("The Minecraft asset-index ID is invalid.".to_string());
    }
    let relative = format!("assets/indexes/{index_id}.json");
    let spec = artifact_spec(asset_index, Some(relative), None).ok_or_else(|| {
        "The installed Minecraft profile has incomplete asset-index metadata.".to_string()
    })?;
    let index_path = safe_join(profile_game_dir, &spec.relative_path)?;
    copy_local_artifact(
        source_game_dir,
        profile_game_dir,
        &spec,
        "Minecraft asset index",
    )?;
    let index_size = fs::metadata(&index_path)
        .map_err(|error| format!("Could not read Minecraft asset-index details: {error}"))?
        .len();
    if index_size > MAX_ASSET_INDEX_BYTES {
        return Err("The Minecraft asset index is unexpectedly large.".to_string());
    }
    let index_value: Value = serde_json::from_slice(
        &fs::read(&index_path)
            .map_err(|error| format!("Could not read Minecraft asset index: {error}"))?,
    )
    .map_err(|error| format!("Minecraft asset index is invalid: {error}"))?;
    let objects = index_value
        .get("objects")
        .and_then(Value::as_object)
        .ok_or_else(|| "Minecraft asset index has no asset objects.".to_string())?;
    let mut hashes_seen = HashSet::new();
    let mut specs = Vec::with_capacity(objects.len());
    for (name, object) in objects {
        let hash = object
            .get("hash")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Minecraft asset {name} has no checksum."))?;
        if !valid_sha1(hash) {
            return Err(format!("Minecraft asset {name} has an invalid checksum."));
        }
        if !hashes_seen.insert(hash.to_ascii_lowercase()) {
            continue;
        }
        let size = object
            .get("size")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("Minecraft asset {name} has no size."))?;
        let prefix = &hash[..2];
        specs.push((
            ArtifactSpec {
                relative_path: format!("assets/objects/{prefix}/{hash}"),
                url: format!("https://resources.download.minecraft.net/{prefix}/{hash}"),
                fallback_urls: Vec::new(),
                sha1: Some(hash.to_string()),
                size: Some(size),
            },
            name.clone(),
        ));
    }
    let total = specs.len();
    if let Some(app) = app {
        report_progress(
            app,
            "assets",
            "Reusing installed Minecraft assets",
            0,
            total,
            52,
        );
    }
    for (index, (spec, name)) in specs.iter().enumerate() {
        copy_local_artifact(
            source_game_dir,
            destination_game_dir,
            spec,
            &format!("Minecraft asset {name}"),
        )?;
        if let Some(app) = app {
            let count = index + 1;
            if count == total || count % 32 == 0 {
                let percent = if total == 0 {
                    74
                } else {
                    52 + ((count * 22) / total) as u8
                };
                report_progress(
                    app,
                    "assets",
                    "Reusing installed Minecraft assets",
                    count,
                    total,
                    percent,
                );
            }
        }
    }
    Ok(())
}

const MAX_LOADER_INSTALLER_BYTES: u64 = 128 * 1024 * 1024;
const LOADER_INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

fn forge_artifact_version(minecraft: &str, loader_version: &str) -> Result<String, String> {
    if let Some((pinned_minecraft, _)) = loader_version.split_once('-') {
        if pinned_minecraft
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
            && pinned_minecraft.contains('.')
            && pinned_minecraft != minecraft
            && loader_version.starts_with(&format!("{pinned_minecraft}-"))
        {
            return Err(format!(
                "This Forge pack pins Minecraft {pinned_minecraft}, but its manifest pins Minecraft {minecraft}."
            ));
        }
    }
    if loader_version.starts_with(&format!("{minecraft}-")) {
        Ok(loader_version.to_string())
    } else {
        Ok(format!("{minecraft}-{loader_version}"))
    }
}

fn loader_installer_url(minecraft: &str, loader: &LoaderPin) -> Result<String, String> {
    let version = match loader.kind {
        LoaderKind::Forge => forge_artifact_version(minecraft, &loader.version)?,
        LoaderKind::NeoForge => loader.version.clone(),
        _ => return Err("This loader does not use an installer JAR.".to_string()),
    };
    let encoded_version = encode_path_segment(&version);
    Ok(match loader.kind {
        LoaderKind::Forge => format!(
            "https://maven.minecraftforge.net/net/minecraftforge/forge/{encoded_version}/forge-{encoded_version}-installer.jar"
        ),
        LoaderKind::NeoForge => format!(
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/{encoded_version}/neoforge-{encoded_version}-installer.jar"
        ),
        _ => unreachable!(),
    })
}

fn download_loader_installer(
    client: &Client,
    url: &str,
    destination: &Path,
    loader: &LoaderPin,
) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("Refusing a non-HTTPS mod-loader installer source.".to_string());
    }
    let expected_host = reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .ok_or_else(|| "The mod-loader installer URL is invalid.".to_string())?;
    let checksum_url = format!("{url}.sha1");
    let checksum_response = client
        .get(&checksum_url)
        .timeout(Duration::from_secs(45))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| {
            format!(
                "Could not verify the official {} installer checksum: {error}",
                loader.kind.label()
            )
        })?;
    if checksum_response.url().scheme() != "https"
        || checksum_response
            .url()
            .host_str()
            .is_none_or(|host| !host.eq_ignore_ascii_case(&expected_host))
    {
        return Err(format!(
            "The official {} checksum source redirected outside its HTTPS Maven host.",
            loader.kind.label()
        ));
    }
    let mut checksum_bytes = Vec::new();
    checksum_response
        .take(256)
        .read_to_end(&mut checksum_bytes)
        .map_err(|error| {
            format!(
                "Could not read the official {} installer checksum: {error}",
                loader.kind.label()
            )
        })?;
    let checksum_text = std::str::from_utf8(&checksum_bytes).map_err(|_| {
        format!(
            "The official {} checksum is not valid text.",
            loader.kind.label()
        )
    })?;
    let expected_sha1 = checksum_text
        .split_whitespace()
        .next()
        .filter(|value| valid_sha1(value))
        .ok_or_else(|| {
            format!(
                "The official {} checksum is malformed.",
                loader.kind.label()
            )
        })?;
    let response = client
        .get(url)
        .timeout(Duration::from_secs(120))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| {
            format!(
                "Could not download the official {} installer: {error}",
                loader.kind.label()
            )
        })?;
    if response.url().scheme() != "https"
        || response
            .content_length()
            .is_some_and(|length| length > MAX_LOADER_INSTALLER_BYTES)
    {
        return Err(format!(
            "The official {} installer redirected insecurely or exceeds the 128 MB safety limit.",
            loader.kind.label()
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_LOADER_INSTALLER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            format!(
                "Could not read the official {} installer: {error}",
                loader.kind.label()
            )
        })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_LOADER_INSTALLER_BYTES {
        return Err(format!(
            "The official {} installer is empty or exceeds the 128 MB safety limit.",
            loader.kind.label()
        ));
    }
    if !sha1_bytes(&bytes).eq_ignore_ascii_case(expected_sha1) {
        return Err(format!(
            "The official {} installer failed its Maven SHA-1 integrity check.",
            loader.kind.label()
        ));
    }
    let mut archive = ZipArchive::new(io::Cursor::new(&bytes)).map_err(|error| {
        format!(
            "The official {} installer is not a valid JAR: {error}",
            loader.kind.label()
        )
    })?;
    if archive.by_name("META-INF/MANIFEST.MF").is_err() {
        return Err(format!(
            "The official {} installer JAR has no Java manifest.",
            loader.kind.label()
        ));
    }
    fs::write(destination, bytes).map_err(|error| {
        format!(
            "Could not stage the official {} installer: {error}",
            loader.kind.label()
        )
    })
}

fn installer_log_excerpt(path: &Path) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if length > 8_000 {
        let _ = file.seek(SeekFrom::Start(length - 8_000));
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).trim().to_string()
}

fn run_loader_installer(
    app: Option<&AppHandle>,
    java: &Path,
    installer: &Path,
    game_dir: &Path,
    loader: &LoaderPin,
) -> Result<(), String> {
    let log_path = game_dir.join(".bee-loader-install.log");
    let log = fs::File::create(&log_path).map_err(|error| {
        format!(
            "Could not create a private {} install log: {error}",
            loader.kind.label()
        )
    })?;
    let stdout = log.try_clone().map_err(|error| {
        format!(
            "Could not open the {} install log: {error}",
            loader.kind.label()
        )
    })?;
    let mut child = Command::new(java)
        .arg("-Djava.net.useSystemProxies=true")
        .arg("-Djava.awt.headless=true")
        .arg("-jar")
        .arg(installer)
        .arg("--installClient")
        .arg(game_dir)
        .current_dir(game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| {
            format!(
                "Could not start the official {} installer: {error}",
                loader.kind.label()
            )
        })?;
    let deadline = Instant::now() + LOADER_INSTALL_TIMEOUT;
    let mut last_progress = String::new();
    let mut last_report = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                if let Some(app) = app.filter(|_| last_report.elapsed() >= Duration::from_secs(2)) {
                    let latest_line = installer_log_excerpt(&log_path)
                        .lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("Working in the isolated profile folder")
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(160)
                        .collect::<String>();
                    if latest_line != last_progress {
                        report_progress(
                            app,
                            loader.kind.dependency_key(),
                            format!("{} installer: {latest_line}", loader.kind.label()),
                            0,
                            1,
                            28,
                        );
                        last_progress = latest_line;
                    }
                    last_report = Instant::now();
                }
                thread::sleep(Duration::from_millis(500));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "The official {} installer did not finish within 15 minutes.",
                    loader.kind.label()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "Could not monitor the official {} installer: {error}",
                    loader.kind.label()
                ));
            }
        }
    };
    if !status.success() {
        let excerpt = installer_log_excerpt(&log_path);
        return Err(if excerpt.is_empty() {
            format!(
                "The official {} installer exited with {status}.",
                loader.kind.label()
            )
        } else {
            format!(
                "The official {} installer exited with {status}. {}",
                loader.kind.label(),
                excerpt
            )
        });
    }
    let _ = fs::remove_file(&log_path);
    Ok(())
}

fn install_loader_profile(
    app: Option<&AppHandle>,
    client: &Client,
    game_dir: &Path,
    minecraft: &str,
    loader: &LoaderPin,
    minecraft_metadata: &Value,
) -> Result<String, String> {
    let label = loader.kind.label();
    if let Some(app) = app {
        report_progress(
            app,
            loader.kind.dependency_key(),
            format!("Resolving {label} {}", loader.version),
            0,
            1,
            19,
        );
    }
    match loader.kind {
        LoaderKind::Fabric | LoaderKind::Quilt => {
            let url = match loader.kind {
                LoaderKind::Fabric => format!(
                    "https://meta.fabricmc.net/v2/versions/loader/{}/{}/profile/json",
                    encode_path_segment(minecraft),
                    encode_path_segment(&loader.version)
                ),
                LoaderKind::Quilt => format!(
                    "https://meta.quiltmc.org/v3/versions/loader/{}/{}/profile/json",
                    encode_path_segment(minecraft),
                    encode_path_segment(&loader.version)
                ),
                _ => unreachable!(),
            };
            let mut profile = request_json(
                client,
                &url,
                &format!("{label} {} profile", loader.version),
                None,
                8 * 1024 * 1024,
            )?;
            let profile_id = profile
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{label} did not return a version profile ID."))?
                .to_string();
            ensure_version_id(&profile_id)?;
            if profile.get("inheritsFrom").and_then(Value::as_str) != Some(minecraft) {
                return Err(format!(
                    "The {label} profile does not inherit the modpack's pinned Minecraft version."
                ));
            }
            if profile.get("mainClass").and_then(Value::as_str).is_none()
                || !metadata_has_loader(&profile, minecraft, loader)
            {
                return Err(format!(
                    "The {label} profile is missing its Java entry point or pinned loader library."
                ));
            }
            if let Some(profile_id_field) = profile
                .as_object_mut()
                .and_then(|object| object.get_mut("id"))
            {
                *profile_id_field = Value::String(profile_id.clone());
            }
            let profile_path = safe_join(
                game_dir,
                &format!("versions/{profile_id}/{profile_id}.json"),
            )?;
            if profile_path.exists() {
                let existing = fs::read(&profile_path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
                if existing.as_ref() != Some(&profile) {
                    return Err(format!(
                        "A different {label} profile already exists at {}; it was left unchanged.",
                        profile_path.display()
                    ));
                }
            } else {
                write_json(&profile_path, &profile, &format!("{label} profile"))?;
            }
            Ok(profile_id)
        }
        LoaderKind::Forge | LoaderKind::NeoForge => {
            let url = loader_installer_url(minecraft, loader)?;
            let installer_path = game_dir.join(".bee-loader-installer.jar");
            let install_result = (|| {
                download_loader_installer(client, &url, &installer_path, loader)?;
                let required_java = super::required_java_major(minecraft_metadata);
                let java =
                    super::find_java_with_major(game_dir, required_java).ok_or_else(|| {
                        required_java.map_or_else(
                            || format!("Java is required to install {}.", label),
                            |major| format!("Java {major} is required to install {}.", label),
                        )
                    })?;
                let java = super::java_probe_path(&java);
                if let Some(app) = app {
                    report_progress(
                        app,
                        loader.kind.dependency_key(),
                        format!(
                            "Running the official {label} installer in the isolated pack folder"
                        ),
                        0,
                        1,
                        28,
                    );
                }
                let registry_path = game_dir.join("launcher_profiles.json");
                let previous_registry = match fs::read(&registry_path) {
                    Ok(bytes) => Some(bytes),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => {
                        return Err(format!(
                            "Could not preserve the isolated launch profile registry: {error}"
                        ));
                    }
                };
                fs::write(&registry_path, br#"{"profiles":{}}"#).map_err(|error| {
                    format!("Could not prepare the isolated {label} installer: {error}")
                })?;
                let run_result =
                    run_loader_installer(app, &java, &installer_path, game_dir, loader);
                let restore_result = match previous_registry {
                    Some(bytes) => fs::write(&registry_path, bytes).map_err(|error| {
                        format!("Could not restore the pack's launch profile registry: {error}")
                    }),
                    None => match fs::remove_file(&registry_path) {
                        Ok(()) => Ok(()),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                        Err(error) => Err(format!(
                            "Could not remove the temporary launch profile registry: {error}"
                        )),
                    },
                };
                run_result?;
                restore_result?;
                find_installed_loader_profile(game_dir, minecraft, loader)
                    .map(|(profile_id, _)| profile_id)
                    .map_err(|_| {
                        format!(
                            "The official {label} installer finished, but did not create a profile matching Minecraft {minecraft} and {label} {}.",
                            loader.version
                        )
                    })
            })();
            let _ = fs::remove_file(&installer_path);
            install_result
        }
    }
}

fn reuse_installed_minecraft_base(
    app: Option<&AppHandle>,
    source_game_dir: &Path,
    destination_game_dir: &Path,
    profile_game_dir: &Path,
    minecraft: &str,
) -> Result<Option<Value>, String> {
    let Some(metadata) = super::metadata_at(source_game_dir, minecraft) else {
        return Ok(None);
    };
    if metadata.get("id").and_then(Value::as_str) != Some(minecraft)
        || metadata.get("mainClass").and_then(Value::as_str).is_none()
        || metadata.get("assetIndex").is_none()
        || metadata.pointer("/downloads/client").is_none()
    {
        return Ok(None);
    }
    let chain = super::metadata_chain(source_game_dir, minecraft)?;
    if chain.first().map(|(id, _)| id.as_str()) != Some(minecraft) {
        return Ok(None);
    }
    if let Some(app) = app {
        report_progress(
            app,
            "minecraft",
            format!("Reusing installed Minecraft {minecraft} base files"),
            0,
            1,
            15,
        );
    }
    copy_installed_profile(source_game_dir, destination_game_dir, &chain)?;
    copy_installed_libraries(source_game_dir, destination_game_dir, &chain)?;
    copy_installed_assets(
        None,
        source_game_dir,
        destination_game_dir,
        profile_game_dir,
        &chain,
    )?;
    Ok(Some(metadata))
}

fn install_version_metadata(
    app: Option<&AppHandle>,
    client: &Client,
    game_dir: &Path,
    minecraft: &str,
    loader: Option<&LoaderPin>,
    source_game_dir: Option<&Path>,
    profile_game_dir: &Path,
) -> Result<String, String> {
    if let Some(app) = app {
        report_progress(
            app,
            "minecraft",
            format!("Resolving Minecraft {minecraft}"),
            0,
            1,
            15,
        );
    }
    let local_metadata = if let Some(source_game_dir) = source_game_dir {
        reuse_installed_minecraft_base(app, source_game_dir, game_dir, profile_game_dir, minecraft)?
    } else {
        None
    };
    let minecraft_metadata = if let Some(metadata) = local_metadata {
        metadata
    } else {
        let manifest = minecraft_version_manifest(client, minecraft)?;
        let versions = manifest
            .get("versions")
            .and_then(Value::as_array)
            .ok_or_else(|| "The official Minecraft version list is incomplete.".to_string())?;
        let entry = versions
            .iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(minecraft))
            .ok_or_else(|| {
                format!("Minecraft {minecraft} is not listed by Mojang's version service.")
            })?;
        let metadata_url = entry
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Minecraft {minecraft} has no version metadata URL."))?;
        let expected_sha1 = entry.get("sha1").and_then(Value::as_str);
        let minecraft_sources = mojang_version_metadata_sources(metadata_url)?;
        request_json_from_sources(
            client,
            &minecraft_sources,
            &format!("Minecraft {minecraft} metadata"),
            expected_sha1,
            16 * 1024 * 1024,
        )?
    };
    if minecraft_metadata.get("id").and_then(Value::as_str) != Some(minecraft) {
        return Err("Mojang returned metadata for a different Minecraft version.".to_string());
    }
    let minecraft_path = safe_join(game_dir, &format!("versions/{minecraft}/{minecraft}.json"))?;
    if minecraft_path.exists() {
        if super::metadata_at(game_dir, minecraft).as_ref() != Some(&minecraft_metadata) {
            return Err(format!(
                "A different Minecraft {minecraft} profile already exists at {}; it was left unchanged.",
                minecraft_path.display()
            ));
        }
    } else {
        write_json(
            &minecraft_path,
            &minecraft_metadata,
            "Minecraft version metadata",
        )?;
    }
    if let Some(loader) = loader {
        if let Ok((profile_id, _)) = find_installed_loader_profile(game_dir, minecraft, loader) {
            return Ok(profile_id);
        }
        install_loader_profile(
            app,
            client,
            game_dir,
            minecraft,
            loader,
            &minecraft_metadata,
        )
    } else {
        if let Some(app) = app {
            report_progress(
                app,
                "minecraft",
                format!("Minecraft {minecraft} metadata ready"),
                1,
                1,
                18,
            );
        }
        Ok(minecraft.to_string())
    }
}

fn sha512_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|error| format!("Could not verify {}: {error}", path.display()))?;
    let mut digest = Sha512::new();
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

fn download_pack_file(client: &Client, game_dir: &Path, file: &ModrinthFile) -> Result<(), String> {
    let destination = safe_join(game_dir, &file.path)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create the folder for {}: {error}", file.path))?;
    }
    let temporary = destination.with_extension("bee-download");
    let mut failures = Vec::new();
    for url in &file.downloads {
        if !url.starts_with("https://") {
            failures.push(format!("Refusing a non-HTTPS URL: {url}"));
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
        if response.url().scheme() != "https" {
            failures.push(format!("{url}: redirected to a non-HTTPS source"));
            continue;
        }
        let result = (|| {
            let output = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temporary)
                .map_err(|error| {
                    format!(
                        "Could not create a temporary download for {}: {error}",
                        file.path
                    )
                })?;
            let mut limited = response.by_ref().take(file.file_size.saturating_add(1));
            let mut output = output;
            let bytes = io::copy(&mut limited, &mut output)
                .map_err(|error| format!("Could not download {}: {error}", file.path))?;
            output
                .sync_all()
                .map_err(|error| format!("Could not finish downloading {}: {error}", file.path))?;
            if bytes != file.file_size {
                return Err(format!(
                    "Downloaded {} has the wrong size ({} bytes, expected {}).",
                    file.path, bytes, file.file_size
                ));
            }
            let actual_sha1 = sha1_file(&temporary)?;
            if !actual_sha1.eq_ignore_ascii_case(&file.hashes.sha1) {
                return Err(format!("Downloaded {} failed its SHA-1 check.", file.path));
            }
            if let Some(expected) = file.hashes.sha512.as_deref() {
                if !sha512_file(&temporary)?.eq_ignore_ascii_case(expected) {
                    return Err(format!(
                        "Downloaded {} failed its SHA-512 check.",
                        file.path
                    ));
                }
            }
            if destination.exists() {
                fs::remove_file(&destination)
                    .map_err(|error| format!("Could not replace {}: {error}", file.path))?;
            }
            fs::rename(&temporary, &destination)
                .map_err(|error| format!("Could not install {}: {error}", file.path))?;
            Ok(())
        })();
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                failures.push(format!("{url}: {error}"));
                let _ = fs::remove_file(&temporary);
            }
        }
    }
    Err(format!(
        "Could not download {} from the modpack's sources. {}",
        file.path,
        failures.join(" | ")
    ))
}

fn curseforge_destinations(
    pack_path: &Path,
    dependency_count: usize,
) -> Result<Vec<CurseForgeDestination>, String> {
    let file = fs::File::open(pack_path)
        .map_err(|error| format!("Could not inspect the CurseForge ZIP: {error}"))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("Could not inspect the CurseForge ZIP: {error}"))?;
    let names: Vec<String> = archive
        .file_names()
        .filter(|name| *name == "modlist.html" || name.ends_with("/modlist.html"))
        .map(str::to_string)
        .collect();
    if names.len() > 1 {
        return Err("This CurseForge ZIP contains more than one modlist.html.".to_string());
    }
    let Some(name) = names.first() else {
        return Ok(vec![CurseForgeDestination::Mods; dependency_count]);
    };
    let mut entry = archive
        .by_name(name)
        .map_err(|error| format!("Could not read the CurseForge mod list: {error}"))?;
    if entry.size() > 8 * 1024 * 1024 {
        return Err("The CurseForge mod list is unexpectedly large.".to_string());
    }
    let mut html = String::with_capacity(entry.size() as usize);
    entry
        .read_to_string(&mut html)
        .map_err(|error| format!("Could not read the CurseForge mod list: {error}"))?;
    let mut destinations = Vec::new();
    for anchor in html.split("<a ").skip(1) {
        let Some(href_at) = anchor.find("href=") else {
            continue;
        };
        let value = anchor[href_at + 5..].trim_start();
        let Some(quote) = value
            .chars()
            .next()
            .filter(|quote| *quote == '\'' || *quote == '"')
        else {
            continue;
        };
        let Some(end) = value[1..].find(quote) else {
            continue;
        };
        let url = &value[1..end + 1];
        let lower = url.to_ascii_lowercase();
        let destination = if lower.contains("/minecraft/texture-packs/") {
            Some(CurseForgeDestination::Resourcepacks)
        } else if lower.contains("/minecraft/shaders/") {
            Some(CurseForgeDestination::Shaderpacks)
        } else if lower.contains("/minecraft/mc-mods/") {
            Some(CurseForgeDestination::Mods)
        } else {
            None
        };
        if let Some(destination) = destination {
            destinations.push(destination);
        }
    }
    if destinations.len() != dependency_count {
        return Err(format!(
            "The CurseForge mod list has {} classifiable entries but the manifest lists {dependency_count} dependencies. Bee Launcher will not guess where ZIP files belong.",
            destinations.len()
        ));
    }
    Ok(destinations)
}

fn safe_curseforge_filename(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.ends_with('.')
        || value.ends_with(' ')
        || value.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '"' | '<' | '>' | '|' | '?' | '*'
                )
        })
    {
        return Err("CurseForge returned an unsafe download filename.".to_string());
    }
    let stem = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
    {
        return Err("CurseForge returned a reserved Windows filename.".to_string());
    }
    Ok(())
}

fn resolve_curseforge_files(
    app: &AppHandle,
    client: &Client,
    pack_path: &Path,
    manifest: &CurseForgeManifest,
    warnings: &mut Vec<String>,
) -> Result<Vec<ResolvedCurseForgeFile>, String> {
    let destinations = curseforge_destinations(pack_path, manifest.files.len())?;
    let required: Vec<(usize, &CurseForgeDependency)> = manifest
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.required)
        .collect();
    if required.is_empty() {
        report_progress(
            app,
            "dependencies",
            "This CurseForge pack has no required files",
            0,
            0,
            77,
        );
        return Ok(Vec::new());
    }
    let mut resolved = Vec::with_capacity(required.len());
    let mut seen_paths = HashSet::new();
    let mut total_bytes = 0_u64;
    let mut untagged_versions = Vec::new();
    for (index, (manifest_index, dependency)) in required.iter().enumerate() {
        let metadata_url = format!(
            "https://www.curseforge.com/api/v1/mods/{}/files/{}",
            dependency.project_id, dependency.file_id
        );
        let value = request_json(
            client,
            &metadata_url,
            &format!("CurseForge file {} metadata", dependency.file_id),
            None,
            2 * 1024 * 1024,
        )?;
        let response: CurseForgeFileResponse = serde_json::from_value(value)
            .map_err(|error| format!("CurseForge returned incomplete file metadata: {error}"))?;
        let file = response.data;
        if file.id != dependency.file_id || file.project_id != dependency.project_id {
            return Err(format!(
                "CurseForge returned a different file for project {} file {}.",
                dependency.project_id, dependency.file_id
            ));
        }
        if file.file_length == 0 || file.file_length > MAX_PACK_FILE_BYTES {
            return Err(format!(
                "CurseForge file {} has an invalid size.",
                dependency.file_id
            ));
        }
        total_bytes = total_bytes.saturating_add(file.file_length);
        if total_bytes > MAX_TOTAL_PACK_FILE_BYTES {
            return Err(
                "The CurseForge dependencies exceed Bee Launcher's 20 GB import limit.".to_string(),
            );
        }
        if file.is_compatible_with_client == Some(false) {
            return Err(format!(
                "CurseForge file {} is explicitly marked as incompatible with Minecraft {} on the client.",
                dependency.file_id, manifest.minecraft.version
            ));
        }
        if !file
            .game_versions
            .iter()
            .any(|version| version == &manifest.minecraft.version)
        {
            untagged_versions.push(file.file_name.clone());
        }
        safe_curseforge_filename(&file.file_name)?;
        let root = match destinations[*manifest_index] {
            CurseForgeDestination::Mods => "mods",
            CurseForgeDestination::Resourcepacks => "resourcepacks",
            CurseForgeDestination::Shaderpacks => "shaderpacks",
        };
        let relative_path = format!("{root}/{}", file.file_name);
        if !seen_paths.insert(relative_path.to_ascii_lowercase()) {
            return Err(format!(
                "Two CurseForge dependencies would overwrite {relative_path}."
            ));
        }
        resolved.push(ResolvedCurseForgeFile {
            project_id: dependency.project_id,
            file_id: dependency.file_id,
            file_name: file.file_name,
            file_length: file.file_length,
            download_url: format!(
                "https://www.curseforge.com/api/v1/mods/{}/files/{}/download",
                dependency.project_id, dependency.file_id
            ),
            relative_path,
        });
        report_progress(
            app,
            "dependencies",
            format!("Resolved CurseForge file {}/{}", index + 1, required.len()),
            index + 1,
            required.len(),
            42 + (((index + 1) * 20) / required.len()) as u8,
        );
    }
    if !untagged_versions.is_empty() {
        let shown = untagged_versions
            .iter()
            .take(6)
            .cloned()
            .collect::<Vec<_>>();
        let names = if untagged_versions.len() > shown.len() {
            format!(
                "{}, and {} more",
                shown.join(", "),
                untagged_versions.len() - shown.len()
            )
        } else {
            shown.join(", ")
        };
        warnings.push(format!(
            "CurseForge marks the pinned files as client-compatible, but does not list Minecraft {} for: {}. The pack's exact pinned files were kept; check the game for compatibility issues.",
            manifest.minecraft.version, names
        ));
    }
    Ok(resolved)
}

fn install_curseforge_file(
    client: &Client,
    profile_dir: &Path,
    file: &ResolvedCurseForgeFile,
) -> Result<(), String> {
    let destination = safe_join(profile_dir, &file.relative_path)?;
    let parent = destination
        .parent()
        .ok_or_else(|| format!("Invalid destination for {}.", file.file_name))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not prepare {}: {error}", file.relative_path))?;
    let temporary = parent.join(format!(
        ".bee-curseforge-{}-{}.download",
        file.project_id, file.file_id
    ));
    let result = (|| {
        let mut response = client
            .get(&file.download_url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| format!("Could not download {}: {error}", file.file_name))?;
        let url = response.url();
        if url.scheme() != "https"
            || !matches!(
                url.host_str(),
                Some("www.curseforge.com" | "mediafilez.forgecdn.net" | "edge.forgecdn.net")
            )
        {
            return Err(format!(
                "CurseForge file {} redirected to an untrusted download host.",
                file.file_id
            ));
        }
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("Could not stage {}: {error}", file.file_name))?;
        let mut output = output;
        let bytes = io::copy(
            &mut response.by_ref().take(file.file_length.saturating_add(1)),
            &mut output,
        )
        .map_err(|error| format!("Could not save {}: {error}", file.file_name))?;
        output
            .sync_all()
            .map_err(|error| format!("Could not finish saving {}: {error}", file.file_name))?;
        if bytes != file.file_length {
            return Err(format!(
                "CurseForge file {} had {} bytes; expected {}.",
                file.file_id, bytes, file.file_length
            ));
        }
        if destination.exists() {
            if !files_have_same_bytes(&temporary, &destination).map_err(|error| {
                format!("Could not compare the existing {}: {error}", file.file_name)
            })? {
                return Err(format!(
                    "The profile already contains a different file named {}; it was not replaced.",
                    file.file_name
                ));
            }
            fs::remove_file(&temporary).map_err(|error| {
                format!("Could not finish checking {}: {error}", file.file_name)
            })?;
        } else {
            fs::rename(&temporary, &destination)
                .map_err(|error| format!("Could not install {}: {error}", file.file_name))?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn install_curseforge_files(
    app: &AppHandle,
    client: &Client,
    profile_dir: &Path,
    files: &[ResolvedCurseForgeFile],
) -> Result<usize, String> {
    if files.is_empty() {
        return Ok(0);
    }
    let queue = Mutex::new(VecDeque::from(files.to_vec()));
    let completed = AtomicUsize::new(0);
    let failure = Mutex::new(None::<String>);
    let worker_count = files.len().min(4);
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let client = client.clone();
            let queue = &queue;
            let completed = &completed;
            let failure = &failure;
            let profile_dir = profile_dir.to_path_buf();
            let app = app.clone();
            scope.spawn(move || loop {
                let file = queue.lock().ok().and_then(|mut queue| queue.pop_front());
                let Some(file) = file else { break };
                if let Err(error) = install_curseforge_file(&client, &profile_dir, &file) {
                    if let Ok(mut first_error) = failure.lock() {
                        if first_error.is_none() {
                            *first_error = Some(error);
                        }
                    }
                    continue;
                }
                let count = completed.fetch_add(1, Ordering::SeqCst) + 1;
                report_progress(
                    &app,
                    "dependencies",
                    format!("Installed {}", file.file_name),
                    count,
                    files.len(),
                    64 + ((count * 30) / files.len()) as u8,
                );
            });
        }
    });
    if let Some(error) = failure.into_inner().unwrap_or(None) {
        return Err(error);
    }
    Ok(files.len())
}

fn local_bundle_entries(
    pack_path: &Path,
    files: &[ModrinthFile],
) -> Result<HashMap<String, String>, String> {
    let file = fs::File::open(pack_path)
        .map_err(|error| format!("Could not open the ZIP bundle: {error}"))?;
    let archive = ZipArchive::new(file)
        .map_err(|error| format!("Could not inspect the ZIP bundle: {error}"))?;
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    let mut entries = HashMap::new();
    for pack_file in files {
        let suffix = format!("/{}", pack_file.path);
        let matches: Vec<&String> = names
            .iter()
            .filter(|name| name.as_str() == pack_file.path || name.ends_with(&suffix))
            .collect();
        if matches.len() > 1 {
            return Err(format!(
                "The ZIP bundle contains multiple copies of {}.",
                pack_file.path
            ));
        }
        if let Some(name) = matches.first() {
            entries.insert(pack_file.path.clone(), (*name).clone());
        }
    }
    Ok(entries)
}

fn extract_bundle_file(
    pack_path: &Path,
    archive_name: &str,
    game_dir: &Path,
    pack_file: &ModrinthFile,
) -> Result<(), String> {
    let file = fs::File::open(pack_path)
        .map_err(|error| format!("Could not open the Bee ZIP bundle: {error}"))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("Could not read the Bee ZIP bundle: {error}"))?;
    let mut entry = archive
        .by_name(archive_name)
        .map_err(|error| format!("The Bee ZIP bundle is missing {}: {error}", pack_file.path))?;
    if entry.is_dir() || entry.size() != pack_file.file_size || entry.size() > MAX_PACK_FILE_BYTES {
        return Err(format!(
            "The bundled copy of {} has an unexpected size or file type.",
            pack_file.path
        ));
    }
    if entry
        .unix_mode()
        .is_some_and(|mode| mode & 0o170000 == 0o120000)
        || entry.enclosed_name().is_none()
    {
        return Err(format!(
            "The Bee ZIP bundle contains an unsafe entry for {}.",
            pack_file.path
        ));
    }
    let destination = safe_join(game_dir, &pack_file.path)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Could not create the folder for {}: {error}",
                pack_file.path
            )
        })?;
    }
    let temporary = destination.with_extension("bee-bundle");
    let mut output = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|error| format!("Could not stage bundled file {}: {error}", pack_file.path))?;
    let copied = io::copy(&mut entry, &mut output)
        .map_err(|error| format!("Could not extract bundled file {}: {error}", pack_file.path))?;
    output
        .sync_all()
        .map_err(|error| format!("Could not finish extracting {}: {error}", pack_file.path))?;
    if copied != pack_file.file_size
        || !sha1_file(&temporary)?.eq_ignore_ascii_case(&pack_file.hashes.sha1)
    {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "The bundled copy of {} failed its size or SHA-1 integrity check.",
            pack_file.path
        ));
    }
    if let Some(expected) = pack_file.hashes.sha512.as_deref() {
        if !sha512_file(&temporary)?.eq_ignore_ascii_case(expected) {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "The bundled copy of {} failed its SHA-512 integrity check.",
                pack_file.path
            ));
        }
    }
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("Could not install bundled file {}: {error}", pack_file.path))
}

fn install_pack_files(
    app: &AppHandle,
    client: &Client,
    game_dir: &Path,
    files: &[ModrinthFile],
    source_path: &Path,
    local_entries: &HashMap<String, String>,
    require_local_files: bool,
) -> Result<usize, String> {
    let files: Vec<ModrinthFile> = files
        .iter()
        .filter(|file| {
            file.env
                .as_ref()
                .and_then(|environment| environment.client.as_deref())
                != Some("unsupported")
        })
        .cloned()
        .collect();
    let total = files.len();
    if total == 0 {
        report_progress(app, "mods", "This pack has no client mod files", 0, 0, 96);
        return Ok(0);
    }
    let queue = Mutex::new(VecDeque::from(files));
    let completed = AtomicUsize::new(0);
    let failure = Mutex::new(None::<String>);
    let worker_count = total.min(4);
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let client = client.clone();
            let queue = &queue;
            let completed = &completed;
            let failure = &failure;
            let game_dir = game_dir.to_path_buf();
            let app = app.clone();
            scope.spawn(move || loop {
                let file = queue.lock().ok().and_then(|mut queue| queue.pop_front());
                let Some(file) = file else {
                    break;
                };
                let install = if let Some(archive_name) = local_entries.get(&file.path) {
                    extract_bundle_file(source_path, archive_name, &game_dir, &file)
                } else if require_local_files {
                    Err(format!(
                        "The Bee ZIP bundle does not include the required mod file {}.",
                        file.path
                    ))
                } else {
                    download_pack_file(&client, &game_dir, &file)
                };
                if let Err(error) = install {
                    if let Ok(mut first_error) = failure.lock() {
                        if first_error.is_none() {
                            *first_error = Some(error);
                        }
                    }
                    continue;
                }
                let count = completed.fetch_add(1, Ordering::SeqCst) + 1;
                if count == total || count % 2 == 0 {
                    let percent = 76 + ((count * 19) / total) as u8;
                    report_progress(
                        &app,
                        "mods",
                        format!("Downloaded {}", file.path),
                        count,
                        total,
                        percent,
                    );
                }
            });
        }
    });
    if let Some(error) = failure.into_inner().unwrap_or(None) {
        return Err(error);
    }
    Ok(total)
}

fn install_assets(
    app: &AppHandle,
    client: &Client,
    profile_game_dir: &Path,
    shared_game_dir: &Path,
    chain: &[(String, Value)],
) -> Result<(), String> {
    let asset_index = chain
        .iter()
        .find_map(|(_, metadata)| metadata.get("assetIndex"))
        .ok_or_else(|| "The pinned Minecraft version has no asset-index metadata.".to_string())?;
    let index_id = asset_index
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "The Minecraft asset index has no ID.".to_string())?;
    if index_id.is_empty()
        || !index_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("The Minecraft asset-index ID is invalid.".to_string());
    }
    let relative = format!("assets/indexes/{index_id}.json");
    let spec = artifact_spec(asset_index, Some(relative), None)
        .ok_or_else(|| "The Minecraft asset index has no HTTPS download URL.".to_string())?;
    report_progress(app, "assets", "Downloading Minecraft asset index", 0, 1, 52);
    let index_path = ensure_downloaded(client, profile_game_dir, &spec, "Minecraft asset index")?;
    let index_size = fs::metadata(&index_path)
        .map_err(|error| format!("Could not read Minecraft asset index details: {error}"))?
        .len();
    if index_size > MAX_ASSET_INDEX_BYTES {
        return Err("The Minecraft asset index is unexpectedly large.".to_string());
    }
    let index_value: Value = serde_json::from_slice(
        &fs::read(&index_path)
            .map_err(|error| format!("Could not read Minecraft asset index: {error}"))?,
    )
    .map_err(|error| format!("Minecraft asset index is invalid: {error}"))?;
    let objects = index_value
        .get("objects")
        .and_then(Value::as_object)
        .ok_or_else(|| "Minecraft asset index has no asset objects.".to_string())?;
    let mut hashes_seen = HashSet::new();
    let mut specs = Vec::with_capacity(objects.len());
    for (name, object) in objects {
        let hash = object
            .get("hash")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Minecraft asset {name} has no checksum."))?;
        if !valid_sha1(hash) {
            return Err(format!("Minecraft asset {name} has an invalid checksum."));
        }
        if !hashes_seen.insert(hash.to_ascii_lowercase()) {
            continue;
        }
        let size = object
            .get("size")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("Minecraft asset {name} has no size."))?;
        let prefix = &hash[..2];
        specs.push(ArtifactSpec {
            relative_path: format!("assets/objects/{prefix}/{hash}"),
            url: format!("https://resources.download.minecraft.net/{prefix}/{hash}"),
            fallback_urls: Vec::new(),
            sha1: Some(hash.to_string()),
            size: Some(size),
        });
    }
    let total = specs.len();
    if total == 0 {
        return Ok(());
    }
    let queue = Mutex::new(VecDeque::from(specs));
    let completed = AtomicUsize::new(0);
    let failure = Mutex::new(None::<String>);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let client = client.clone();
            let queue = &queue;
            let completed = &completed;
            let failure = &failure;
            let game_dir = shared_game_dir.to_path_buf();
            let app = app.clone();
            scope.spawn(move || loop {
                let spec = queue.lock().ok().and_then(|mut queue| queue.pop_front());
                let Some(spec) = spec else {
                    break;
                };
                if let Err(error) = ensure_downloaded(&client, &game_dir, &spec, "Minecraft asset")
                {
                    if let Ok(mut first_error) = failure.lock() {
                        if first_error.is_none() {
                            *first_error = Some(error);
                        }
                    }
                    continue;
                }
                let count = completed.fetch_add(1, Ordering::SeqCst) + 1;
                if count == total || count % 32 == 0 {
                    let percent = 54 + ((count * 20) / total) as u8;
                    report_progress(
                        &app,
                        "assets",
                        "Preparing Minecraft assets",
                        count,
                        total,
                        percent,
                    );
                }
            });
        }
    });
    if let Some(error) = failure.into_inner().unwrap_or(None) {
        return Err(error);
    }
    Ok(())
}

fn parse_marker(path: &Path) -> Option<InstanceMarker> {
    let marker = fs::read(path.join(".bee-instance.json")).ok()?;
    let instance: InstanceMarker = serde_json::from_slice(&marker).ok()?;
    if !safe_instance_id(&instance.instance_id) || ensure_version_id(&instance.profile_id).is_err()
    {
        return None;
    }
    Some(instance)
}

fn safe_instance_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

pub(super) fn installed_profiles(app: &AppHandle) -> Vec<InstalledVersion> {
    let Ok(instances) = app_instances_dir(app) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(instances) else {
        return Vec::new();
    };
    let shared_dir = app_shared_game_files_dir(app).ok();
    let _store_guard = SHARED_GAME_FILES_LOCK.lock().ok();
    let mut profiles = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(marker) = parse_marker(&path) else {
            continue;
        };
        if marker.instance_id != entry.file_name().to_string_lossy() {
            continue;
        }
        if let Some(shared_dir) = shared_dir.as_deref() {
            // Older releases stored these immutable game files in each pack. Merge only
            // byte-identical paths; on conflicts, keep the original profile untouched.
            let _ = ensure_profile_shared_game_files(&path, shared_dir);
        }
        for mut profile in super::installed_versions(&path) {
            if profile.id != marker.profile_id {
                continue;
            }
            profile.profile_key = format!("pack:{}:{}", marker.instance_id, profile.id);
            profile.instance_name = Some(marker.name.clone());
            profiles.push(profile);
        }
    }
    profiles.sort_by(|left, right| {
        left.instance_name
            .cmp(&right.instance_name)
            .then_with(|| left.id.cmp(&right.id))
    });
    profiles
}

pub(super) fn resolve_profile(
    app: &AppHandle,
    profile_key: &str,
) -> Result<(PathBuf, String), String> {
    let mut parts = profile_key.splitn(3, ':');
    let kind = parts.next().unwrap_or("");
    match kind {
        "minecraft" => {
            let version_id = parts.next().unwrap_or("");
            ensure_version_id(version_id)?;
            Ok((super::resolve_game_dir(app)?, version_id.to_string()))
        }
        "pack" => {
            let instance_id = parts.next().unwrap_or("");
            let version_id = parts.next().unwrap_or("");
            if !safe_instance_id(instance_id) {
                return Err("That modpack instance key is invalid.".to_string());
            }
            ensure_version_id(version_id)?;
            let instance_dir = app_instances_dir(app)?.join(instance_id);
            let marker = parse_marker(&instance_dir).ok_or_else(|| {
                "That imported modpack instance is missing or incomplete.".to_string()
            })?;
            if marker.instance_id != instance_id || marker.profile_id != version_id {
                return Err(
                    "That modpack profile key does not match its installed instance.".to_string(),
                );
            }
            if let Ok(shared_dir) = app_shared_game_files_dir(app) {
                if let Ok(_guard) = SHARED_GAME_FILES_LOCK.lock() {
                    // A collision in an older profile is non-destructive: that profile
                    // continues using its local runtime while new imports share the store.
                    let _ = ensure_profile_shared_game_files(&instance_dir, &shared_dir);
                }
            }
            if !super::metadata_at(&instance_dir, version_id).is_some() {
                return Err("That modpack's Minecraft profile files are incomplete.".to_string());
            }
            Ok((instance_dir, version_id.to_string()))
        }
        _ => Err(
            "That Minecraft profile selection is invalid. Refresh the profile list and try again."
                .to_string(),
        ),
    }
}

pub(super) fn import_modpack(app: &AppHandle, source: &str) -> Result<ImportResult, String> {
    let pack_path = PathBuf::from(source);
    let extension = pack_path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    let is_zip = extension.eq_ignore_ascii_case("zip");
    if !is_zip && !extension.eq_ignore_ascii_case("mrpack") {
        return Err(
            "Choose a Modrinth .mrpack file or a Modrinth/CurseForge ZIP export.".to_string(),
        );
    }
    report_progress(app, "validate", "Reading modpack manifest", 0, 1, 2);
    let manifest = read_any_manifest(&pack_path)?;
    let (details, is_curseforge) = match &manifest {
        PackManifest::Modrinth(manifest) => {
            let (minecraft_version, loader) = validate_manifest(manifest)?;
            (
                PackDetails {
                    name: manifest.name.clone(),
                    version: manifest.version_id.clone(),
                    minecraft_version,
                    loader,
                    recommended_ram_mb: None,
                },
                false,
            )
        }
        PackManifest::CurseForge(manifest) => (validate_curseforge_manifest(manifest)?, true),
    };
    let is_bundle = is_zip && !is_curseforge;
    let local_entries = if let PackManifest::Modrinth(manifest) = &manifest {
        if is_bundle {
            local_bundle_entries(&pack_path, &manifest.files)?
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };
    if let PackManifest::Modrinth(manifest) = &manifest {
        let client_file_count = manifest
            .files
            .iter()
            .filter(|file| {
                file.env
                    .as_ref()
                    .and_then(|environment| environment.client.as_deref())
                    != Some("unsupported")
            })
            .count();
        if is_bundle && local_entries.len() < client_file_count {
            return Err("This Bee ZIP does not contain every client mod listed in its Modrinth manifest. Choose the original .mrpack or a complete Bee bundle instead.".to_string());
        }
    }
    let installed_game_dir = Some(super::resolve_game_dir(app)?);
    let instances = app_instances_dir(app)?;
    let (instance_id, instance_dir) = allocate_instance_dir(&instances, &details.name)?;
    let _store_guard = SHARED_GAME_FILES_LOCK
        .lock()
        .map_err(|_| "The shared Minecraft file store is temporarily unavailable.".to_string())?;
    let common_dir = app_shared_game_files_dir(app)?;
    let mut warnings = Vec::new();
    let result = (|| {
        report_progress(
            app,
            "validate",
            format!("Preparing {} as a separate profile", details.name),
            1,
            1,
            6,
        );
        copy_overrides(&pack_path, &instance_dir, app)?;
        let client = downloader()?;
        let reusable = installed_game_dir.as_deref().and_then(|source_game_dir| {
            find_installed_pack_profile(
                source_game_dir,
                &details.minecraft_version,
                details.loader.as_ref(),
            )
            .ok()
            .map(|(profile_id, chain)| (source_game_dir, profile_id, chain))
        });
        if is_bundle && reusable.is_none() {
            let loader = details
                .loader
                .as_ref()
                .map(|loader| format!(" with {} {}", loader.kind.label(), loader.version))
                .unwrap_or_default();
            return Err(format!(
                "This Bee ZIP needs Minecraft {}{loader}, but that profile is not installed in the selected .minecraft folder. Install and launch it once, then retry.",
                details.minecraft_version
            ));
        }
        if let Some(source_root) = installed_game_dir.as_deref() {
            report_progress(
                app,
                "minecraft",
                "Copying .minecraft assets, libraries, and versions into Bee's shared store",
                0,
                1,
                8,
            );
            let seed = seed_shared_game_store_from_local_base(source_root, &common_dir)?;
            if seed.conflicts_left_unchanged > 0 {
                warnings.push(format!(
                    "{} pre-existing shared Minecraft file(s) differed from the selected .minecraft base and were left unchanged.",
                    seed.conflicts_left_unchanged
                ));
            }
            let message = if seed.already_seeded {
                "Bee's shared Minecraft base is already copied; reusing it"
            } else if seed.copied_files > 0 {
                "Copied the selected .minecraft base into Bee's shared store"
            } else {
                "No local Minecraft base folders needed copying"
            };
            report_progress(app, "minecraft", message, 1, 1, 14);
        }
        let profile_id = if let Some((source_game_dir, profile_id, source_chain)) = reusable {
            report_progress(
                app,
                "minecraft",
                format!(
                    "Reusing matching Minecraft {} files",
                    details.minecraft_version
                ),
                0,
                1,
                15,
            );
            copy_installed_profile(source_game_dir, &common_dir, &source_chain)?;
            let chain = super::metadata_chain(&common_dir, &profile_id)?;
            report_progress(
                app,
                "minecraft",
                format!("Sharing installed {profile_id} profile and libraries"),
                0,
                1,
                25,
            );
            copy_installed_libraries(source_game_dir, &common_dir, &chain)?;
            let _ = super::install_bundled_authlib(&common_dir, &chain)?;
            copy_installed_assets(
                Some(app),
                source_game_dir,
                &common_dir,
                &instance_dir,
                &chain,
            )?;
            report_progress(
                app,
                "minecraft",
                "Minecraft files added to the shared store",
                1,
                1,
                52,
            );
            profile_id
        } else {
            if is_curseforge {
                report_progress(
                    app,
                    "minecraft",
                    "No matching local Minecraft profile; preparing it online",
                    0,
                    1,
                    15,
                );
            }
            let profile_id = install_version_metadata(
                Some(app),
                &client,
                &common_dir,
                &details.minecraft_version,
                details.loader.as_ref(),
                installed_game_dir.as_deref(),
                &instance_dir,
            )?;
            report_progress(
                app,
                "minecraft",
                "Preparing Minecraft client and libraries",
                0,
                1,
                30,
            );
            let chain = super::metadata_chain(&common_dir, &profile_id)?;
            super::ensure_client_jar(&client, &common_dir, &chain)?;
            super::ensure_library_artifacts(&client, &common_dir, &chain)?;
            let _ = super::install_bundled_authlib(&common_dir, &chain)?;
            report_progress(
                app,
                "minecraft",
                "Minecraft client and libraries are ready",
                1,
                1,
                48,
            );
            install_assets(app, &client, &instance_dir, &common_dir, &chain)?;
            profile_id
        };
        ensure_profile_shared_game_files(&instance_dir, &common_dir)?;
        let installed_files = match &manifest {
            PackManifest::Modrinth(manifest) => install_pack_files(
                app,
                &client,
                &instance_dir,
                &manifest.files,
                &pack_path,
                &local_entries,
                is_bundle,
            )?,
            PackManifest::CurseForge(manifest) => {
                let files =
                    resolve_curseforge_files(app, &client, &pack_path, manifest, &mut warnings)?;
                install_curseforge_files(app, &client, &instance_dir, &files)?
            }
        };
        let marker = InstanceMarker {
            instance_id: instance_id.clone(),
            name: details.name.clone(),
            minecraft_version: details.minecraft_version.clone(),
            loader_name: details
                .loader
                .as_ref()
                .map(|item| item.kind.label().to_string()),
            loader_version: details.loader.as_ref().map(|item| item.version.clone()),
            fabric_loader: details
                .loader
                .as_ref()
                .filter(|item| item.kind == LoaderKind::Fabric)
                .map(|item| item.version.clone()),
            profile_id: profile_id.clone(),
            pack_version: details.version.clone(),
        };
        let marker_path = instance_dir.join(".bee-instance.json");
        let marker_bytes = serde_json::to_vec_pretty(&marker)
            .map_err(|error| format!("Could not save modpack instance details: {error}"))?;
        fs::write(&marker_path, marker_bytes)
            .map_err(|error| format!("Could not finish installing the modpack: {error}"))?;
        report_progress(
            app,
            "complete",
            format!("{} installed successfully", details.name),
            installed_files,
            installed_files,
            100,
        );
        Ok(ImportResult {
            instance_name: details.name.clone(),
            profile_key: format!("pack:{instance_id}:{profile_id}"),
            minecraft_version: details.minecraft_version.clone(),
            loader_name: details
                .loader
                .as_ref()
                .map(|item| item.kind.label().to_string()),
            loader_version: details.loader.as_ref().map(|item| item.version.clone()),
            files_installed: installed_files,
            warnings: warnings.clone(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&instance_dir);
    }
    if result.is_ok() {
        if let Some(ram_mb) = details.recommended_ram_mb {
            let profile_key = result
                .as_ref()
                .expect("checked successful result")
                .profile_key
                .clone();
            let mut settings = super::read_stored_settings(app);
            let recommended_gb = (ram_mb.saturating_add(1023) / 1024).clamp(1, 64) as u8;
            settings
                .profile_settings
                .entry(profile_key)
                .or_default()
                .max_memory_gb = Some(recommended_gb);
            let _ = super::write_stored_settings(app, &settings);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format_v1_manifest_value(loader: Option<(&str, &str)>) -> Value {
        let mut dependencies = serde_json::Map::from_iter([(
            "minecraft".to_string(),
            Value::String("1.20.1".to_string()),
        )]);
        if let Some((key, version)) = loader {
            dependencies.insert(key.to_string(), Value::String(version.to_string()));
        }
        serde_json::json!({
            "formatVersion": 1,
            "game": "minecraft",
            "versionId": "fixture-1",
            "name": "Format fixture",
            "files": [],
            "dependencies": dependencies,
        })
    }

    fn format_v1_manifest(loader: Option<(&str, &str)>) -> ModrinthManifest {
        serde_json::from_value(format_v1_manifest_value(loader))
            .expect("fixture should match the Modrinth format-v1 manifest")
    }

    fn curseforge_fixture_value() -> Value {
        serde_json::json!({
            "manifestType": "minecraftModpack",
            "manifestVersion": 1,
            "name": "Satisfaction Guaranteed",
            "version": "1.2.1",
            "minecraft": {
                "version": "26.3",
                "modLoaders": [{"id": "fabric-0.20.1", "primary": true}],
                "recommendedRam": 6016
            },
            "files": [
                {"projectID": 10, "fileID": 101, "required": true},
                {"projectID": 11, "fileID": 102, "required": true},
                {"projectID": 12, "fileID": 103, "required": false}
            ],
            "overrides": "overrides"
        })
    }

    #[test]
    fn validates_curseforge_loader_pin_and_recommended_memory() {
        let manifest: CurseForgeManifest = serde_json::from_value(curseforge_fixture_value())
            .expect("CurseForge fixture should deserialize");
        let details = validate_curseforge_manifest(&manifest)
            .expect("supported CurseForge manifest should validate");
        assert_eq!(details.name, "Satisfaction Guaranteed");
        assert_eq!(details.version.as_deref(), Some("1.2.1"));
        assert_eq!(details.minecraft_version, "26.3");
        assert_eq!(details.recommended_ram_mb, Some(6016));
        assert_eq!(
            details.loader,
            Some(LoaderPin {
                kind: LoaderKind::Fabric,
                version: "0.20.1".to_string()
            })
        );
    }

    #[test]
    fn classifies_curseforge_mods_texture_packs_and_shaders_in_manifest_order() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        let pack = std::env::temp_dir().join(format!("bee-curseforge-categories-{unique}.zip"));
        let file = fs::File::create(&pack).expect("fixture archive should be created");
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("manifest.json", zip::write::SimpleFileOptions::default())
            .expect("manifest entry should open");
        archive
            .write_all(
                &serde_json::to_vec(&curseforge_fixture_value()).expect("manifest should encode"),
            )
            .expect("manifest should be written");
        archive
            .start_file("modlist.html", zip::write::SimpleFileOptions::default())
            .expect("mod list entry should open");
        archive
            .write_all(
                b"<ul><li><a href=\"https://www.curseforge.com/minecraft/mc-mods/mod-a\">A</a></li><li><a href=\"https://www.curseforge.com/minecraft/texture-packs/pack-b\">B</a></li><li><a href=\"https://www.curseforge.com/minecraft/shaders/shader-c\">C</a></li></ul>",
            )
            .expect("mod list should be written");
        archive.finish().expect("fixture archive should close");
        let parsed = read_any_manifest(&pack).expect("CurseForge ZIP should parse");
        assert!(matches!(parsed, PackManifest::CurseForge(_)));
        assert_eq!(
            curseforge_destinations(&pack, 3).expect("all file types should be found"),
            vec![
                CurseForgeDestination::Mods,
                CurseForgeDestination::Resourcepacks,
                CurseForgeDestination::Shaderpacks
            ]
        );
        fs::remove_file(pack).expect("fixture archive should be removed");
    }

    #[test]
    fn first_local_base_seed_is_physical_and_later_profiles_reuse_it() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bee-local-base-seed-{unique}"));
        let source = root.join("selected-minecraft");
        let common = root.join("bee-data/minecraft-common");
        for directory in [
            "assets/indexes",
            "assets/objects/aa",
            "libraries/com/example",
            "versions/1.20.1",
            "mods",
            "config",
            "saves/world",
        ] {
            fs::create_dir_all(source.join(directory)).expect("fixture folder should be created");
        }
        fs::create_dir_all(&common).expect("shared store should exist");
        fs::write(source.join("assets/indexes/test.json"), b"asset index")
            .expect("asset index should be written");
        fs::write(source.join("assets/objects/aa/asset-hash"), b"asset bytes")
            .expect("asset should be written");
        fs::write(
            source.join("libraries/com/example/library.jar"),
            b"library bytes",
        )
        .expect("library should be written");
        fs::write(
            source.join("versions/1.20.1/1.20.1.json"),
            b"version metadata",
        )
        .expect("version metadata should be written");
        fs::write(
            source.join("versions/1.20.1/1.20.1.jar"),
            b"client jar bytes",
        )
        .expect("client JAR should be written");
        fs::write(source.join("mods/source-only.jar"), b"source mod")
            .expect("source mod should be written");
        fs::write(source.join("config/source-only.json"), b"source config")
            .expect("source config should be written");
        fs::write(source.join("saves/world/level.dat"), b"source world")
            .expect("source save should be written");

        let first = seed_shared_game_store_from_local_base(&source, &common)
            .expect("first import should seed a local Minecraft base");
        assert_eq!(first.copied_files, 5);
        assert!(!first.already_seeded);
        for relative in [
            "assets/indexes/test.json",
            "assets/objects/aa/asset-hash",
            "libraries/com/example/library.jar",
            "versions/1.20.1/1.20.1.json",
            "versions/1.20.1/1.20.1.jar",
        ] {
            let source_file = source.join(relative);
            let shared_file = common.join(relative);
            assert_eq!(
                fs::read(&source_file).expect("source file should remain readable"),
                fs::read(&shared_file).expect("a physical shared copy should exist")
            );
            assert_ne!(
                fs::canonicalize(source_file).expect("source path should resolve"),
                fs::canonicalize(shared_file).expect("shared path should resolve")
            );
        }
        assert!(!common.join("mods").exists());
        assert!(!common.join("config").exists());
        assert!(!common.join("saves").exists());
        assert_eq!(
            fs::read(source.join("mods/source-only.jar")).unwrap(),
            b"source mod"
        );
        assert_eq!(
            fs::read(source.join("saves/world/level.dat")).unwrap(),
            b"source world"
        );

        fs::write(
            source.join("assets/objects/aa/asset-hash"),
            b"changed later",
        )
        .expect("fixture source should be editable after the first import");
        fs::create_dir_all(source.join("assets/objects/bb"))
            .expect("fixture should be able to add a later asset");
        fs::write(source.join("assets/objects/bb/new-asset"), b"new later")
            .expect("fixture should gain a later asset");
        let second = seed_shared_game_store_from_local_base(&source, &common)
            .expect("second import should reuse the original shared base");
        assert!(second.already_seeded);
        assert_eq!(second.copied_files, 0);
        assert_eq!(
            fs::read(common.join("assets/objects/aa/asset-hash")).unwrap(),
            b"asset bytes"
        );
        assert!(!common.join("assets/objects/bb/new-asset").exists());

        let first_profile = root.join("instances/first-pack");
        let second_profile = root.join("instances/second-pack");
        for (profile, suffix) in [(&first_profile, "first"), (&second_profile, "second")] {
            fs::create_dir_all(profile.join("mods")).expect("profile mods should be local");
            fs::create_dir_all(profile.join("config")).expect("profile config should be local");
            fs::create_dir_all(profile.join("saves/world")).expect("profile saves should be local");
            fs::write(profile.join("mods/profile.jar"), suffix.as_bytes())
                .expect("profile mod should be written");
            fs::write(profile.join("config/profile.json"), suffix.as_bytes())
                .expect("profile config should be written");
            fs::write(profile.join("saves/world/level.dat"), suffix.as_bytes())
                .expect("profile save should be written");
            ensure_profile_shared_game_files(profile, &common)
                .expect("imported profile should use Bee's shared files");
            assert!(is_linked_to(
                &profile.join("assets/objects"),
                &common.join("assets/objects")
            ));
            assert!(!is_linked_to(
                &profile.join("assets/indexes"),
                &common.join("assets/indexes")
            ));
            for folder in ["libraries", "versions"] {
                assert!(is_linked_to(&profile.join(folder), &common.join(folder)));
            }
        }
        assert_eq!(
            fs::read(first_profile.join("mods/profile.jar")).unwrap(),
            b"first"
        );
        assert_eq!(
            fs::read(second_profile.join("mods/profile.jar")).unwrap(),
            b"second"
        );
        assert_ne!(
            fs::canonicalize(first_profile.join("mods/profile.jar")).unwrap(),
            fs::canonicalize(second_profile.join("mods/profile.jar")).unwrap()
        );
        fs::remove_dir_all(root).expect("isolated fixture tree should be removed");
    }

    #[test]
    fn shared_store_migration_keeps_mutable_profile_files_local() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bee-shared-game-files-{unique}"));
        let profile = root.join("instances/outbreak");
        let common = root.join("minecraft-common");
        fs::create_dir_all(profile.join("assets/objects/aa")).expect("profile assets should exist");
        fs::create_dir_all(profile.join("libraries/example"))
            .expect("profile libraries should exist");
        fs::create_dir_all(profile.join("versions/fabric-test"))
            .expect("profile versions should exist");
        fs::create_dir_all(profile.join("mods")).expect("profile mods should exist");
        fs::create_dir_all(profile.join("config")).expect("profile config should exist");
        fs::create_dir_all(profile.join("saves/world")).expect("profile saves should exist");
        fs::create_dir_all(common.join("assets/objects/aa")).expect("shared assets should exist");
        fs::write(profile.join("assets/objects/aa/hash"), b"asset")
            .expect("asset should be written");
        fs::write(profile.join("libraries/example/library.jar"), b"library")
            .expect("library should be written");
        fs::write(
            profile.join("versions/fabric-test/profile.json"),
            b"metadata",
        )
        .expect("version metadata should be written");
        fs::write(profile.join("mods/outbreak.jar"), b"outbreak only")
            .expect("mod should be written");
        fs::write(profile.join("config/outbreak.json"), b"profile config")
            .expect("config should be written");
        fs::write(profile.join("saves/world/level.dat"), b"world").expect("save should be written");
        fs::write(common.join("assets/objects/aa/hash"), b"asset")
            .expect("shared copy should be written");

        ensure_profile_shared_game_files(&profile, &common)
            .expect("profile should link to shared files");
        assert!(is_linked_to(
            &profile.join("assets/objects"),
            &common.join("assets/objects")
        ));
        assert!(!is_linked_to(
            &profile.join("assets/indexes"),
            &common.join("assets/indexes")
        ));
        for folder in ["libraries", "versions"] {
            assert!(is_linked_to(&profile.join(folder), &common.join(folder)));
        }
        assert_eq!(
            fs::read(profile.join("mods/outbreak.jar")).unwrap(),
            b"outbreak only"
        );
        assert_eq!(
            fs::read(profile.join("config/outbreak.json")).unwrap(),
            b"profile config"
        );
        assert_eq!(
            fs::read(profile.join("saves/world/level.dat")).unwrap(),
            b"world"
        );
        assert!(!profile.join("assets/.objects.bee-local-backup").exists());
        fs::remove_dir_all(root).expect("fixture tree should be removed");
    }

    #[test]
    fn shared_store_conflict_does_not_replace_existing_profile_or_shared_files() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bee-shared-game-conflict-{unique}"));
        let profile = root.join("instances/smallhold");
        let common = root.join("minecraft-common");
        fs::create_dir_all(profile.join("versions")).expect("profile versions should exist");
        fs::create_dir_all(common.join("versions")).expect("shared versions should exist");
        fs::write(profile.join("versions/version.json"), b"profile version")
            .expect("profile version should be written");
        fs::write(common.join("versions/version.json"), b"shared version")
            .expect("shared version should be written");
        let error = ensure_profile_shared_game_files(&profile, &common)
            .expect_err("conflicting version metadata must not be merged");
        assert!(error.contains("left intact"));
        assert_eq!(
            fs::read(profile.join("versions/version.json")).unwrap(),
            b"profile version"
        );
        assert_eq!(
            fs::read(common.join("versions/version.json")).unwrap(),
            b"shared version"
        );
        assert!(profile.join("versions").is_dir());
        fs::remove_dir_all(root).expect("fixture tree should be removed");
    }

    #[test]
    fn colliding_asset_index_ids_are_isolated_across_repeated_profile_imports() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bee-asset-index-collision-{unique}"));
        let common = root.join("minecraft-common");
        let shared_index = b"an older pack's valid index with the same short ID";
        fs::create_dir_all(common.join("assets/indexes"))
            .expect("shared asset-index directory should be created");
        fs::write(common.join("assets/indexes/34.json"), shared_index)
            .expect("pre-existing shared index should be written");

        for (pack, object_name, asset_bytes) in [
            ("outbreak", "music/menu", b"outbreak asset".as_slice()),
            ("smallhold", "music/calm", b"smallhold asset".as_slice()),
        ] {
            let source = root.join(format!("sources/{pack}"));
            let profile = root.join(format!("instances/{pack}"));
            let asset_hash = sha1_bytes(asset_bytes);
            let index = serde_json::to_vec(&serde_json::json!({
                "objects": {
                    object_name: {
                        "hash": asset_hash,
                        "size": asset_bytes.len()
                    }
                }
            }))
            .expect("pack-specific asset index should encode");
            let source_index = source.join("assets/indexes/34.json");
            fs::create_dir_all(source_index.parent().unwrap())
                .expect("source index directory should be created");
            fs::write(&source_index, &index).expect("source index should be written");
            let source_object = source.join(format!(
                "assets/objects/{}/{}",
                &asset_hash[..2],
                asset_hash
            ));
            fs::create_dir_all(source_object.parent().unwrap())
                .expect("source object directory should be created");
            fs::write(&source_object, asset_bytes).expect("source object should be written");

            let chain = vec![(
                "26.3".to_string(),
                serde_json::json!({
                    "assetIndex": {
                        "id": "34",
                        "url": "https://example.invalid/34.json",
                        "sha1": sha1_bytes(&index),
                        "size": index.len()
                    }
                }),
            )];
            copy_installed_assets(None, &source, &common, &profile, &chain)
                .expect("a conflicting common index must not block a verified local index");
            ensure_profile_shared_game_files(&profile, &common)
                .expect("profile should share objects while keeping its own index");

            let profile_index = profile.join("assets/indexes/34.json");
            assert_eq!(fs::read(&profile_index).unwrap(), index);
            assert_eq!(fs::read(&source_index).unwrap(), index);
            assert_eq!(
                fs::read(common.join("assets/indexes/34.json")).unwrap(),
                shared_index
            );
            assert_eq!(
                fs::read(common.join(format!(
                    "assets/objects/{}/{}",
                    &asset_hash[..2],
                    asset_hash
                )))
                .unwrap(),
                asset_bytes
            );
            assert!(!is_linked_to(
                &profile.join("assets/indexes"),
                &common.join("assets/indexes")
            ));
            assert!(is_linked_to(
                &profile.join("assets/objects"),
                &common.join("assets/objects")
            ));

            // An identical repeat for this instance is idempotent; a second
            // pack with another index body but the same ID stays independent.
            copy_installed_assets(None, &source, &common, &profile, &chain)
                .expect("repeated import should reuse the instance's verified index");
            ensure_profile_shared_game_files(&profile, &common)
                .expect("repeated sharing should remain idempotent");
            assert_eq!(fs::read(&profile_index).unwrap(), index);
        }

        assert_eq!(
            fs::read(common.join("assets/indexes/34.json")).unwrap(),
            shared_index,
            "neither repeated import may replace the shared index"
        );
        fs::remove_dir_all(root).expect("isolated fixture tree should be removed");
    }

    #[test]
    fn accepts_the_format_v1_loader_dependency_keys_and_pins() {
        let cases = [
            ("fabric-loader", "0.16.9", LoaderKind::Fabric),
            ("quilt-loader", "0.30.0", LoaderKind::Quilt),
            ("forge", "47.4.29", LoaderKind::Forge),
            ("neoforge", "21.1.266", LoaderKind::NeoForge),
        ];
        for (key, version, kind) in cases {
            let (minecraft, loader) = validate_manifest(&format_v1_manifest(Some((key, version))))
                .unwrap_or_else(|error| panic!("{key} pack should validate: {error}"));
            assert_eq!(minecraft, "1.20.1");
            assert_eq!(
                loader,
                Some(LoaderPin {
                    kind,
                    version: version.to_string()
                })
            );
        }
        assert_eq!(
            validate_manifest(&format_v1_manifest(None))
                .expect("vanilla format-v1 pack should validate")
                .1,
            None
        );
    }

    #[test]
    fn parses_real_format_v1_mrpack_archives_for_each_supported_loader() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bee-mrpack-format-test-{unique}"));
        fs::create_dir_all(&root).expect("temporary fixture folder should be created");
        for (key, version, expected_kind) in [
            ("fabric-loader", "0.16.9", LoaderKind::Fabric),
            ("quilt-loader", "0.30.0", LoaderKind::Quilt),
            ("forge", "47.4.29", LoaderKind::Forge),
            ("neoforge", "21.1.266", LoaderKind::NeoForge),
        ] {
            let pack = root.join(format!("{key}.mrpack"));
            let file = fs::File::create(&pack).expect("fixture archive should be created");
            let mut archive = zip::ZipWriter::new(file);
            archive
                .start_file(
                    "modrinth.index.json",
                    zip::write::SimpleFileOptions::default(),
                )
                .expect("manifest entry should open");
            archive
                .write_all(
                    &serde_json::to_vec(&format_v1_manifest_value(Some((key, version))))
                        .expect("manifest should serialize"),
                )
                .expect("manifest should be written");
            archive.finish().expect("fixture archive should close");

            let parsed = read_manifest(&pack).expect("format-v1 archive manifest should parse");
            let (minecraft, loader) =
                validate_manifest(&parsed).expect("supported loader manifest should validate");
            assert_eq!(minecraft, "1.20.1");
            assert_eq!(
                loader,
                Some(LoaderPin {
                    kind: expected_kind,
                    version: version.to_string()
                })
            );
        }
        fs::remove_dir_all(root).expect("temporary fixture folder should be removed");
    }

    #[test]
    fn matches_loader_profiles_by_their_exact_pinned_library_coordinate() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bee-loader-profile-match-{unique}"));
        let minecraft = "1.20.1";
        fs::create_dir_all(root.join("versions").join(minecraft))
            .expect("vanilla metadata folder should be created");
        fs::write(
            root.join("versions")
                .join(minecraft)
                .join(format!("{minecraft}.json")),
            serde_json::to_vec(&serde_json::json!({
                "id": minecraft,
                "mainClass": "net.minecraft.client.main.Main"
            }))
            .expect("vanilla metadata should encode"),
        )
        .expect("vanilla metadata should be written");

        let cases = [
            (
                LoaderPin {
                    kind: LoaderKind::Fabric,
                    version: "0.16.9".to_string(),
                },
                "fabric-loader-0.16.9-1.20.1",
                "net.fabricmc:fabric-loader:0.16.9",
            ),
            (
                LoaderPin {
                    kind: LoaderKind::Quilt,
                    version: "0.30.0".to_string(),
                },
                "quilt-loader-0.30.0-1.20.1",
                "org.quiltmc:quilt-loader:0.30.0",
            ),
            (
                LoaderPin {
                    kind: LoaderKind::Forge,
                    version: "47.4.29".to_string(),
                },
                "1.20.1-forge-47.4.29",
                "net.minecraftforge:forge:1.20.1-47.4.29",
            ),
            (
                LoaderPin {
                    kind: LoaderKind::NeoForge,
                    version: "21.1.266".to_string(),
                },
                "neoforge-21.1.266",
                "net.neoforged.fancymodloader:loader:4.0.45",
            ),
        ];
        for (loader, profile_id, coordinate) in cases {
            let profile_dir = root.join("versions").join(profile_id);
            fs::create_dir_all(&profile_dir).expect("mod-loader profile folder should be created");
            let profile = serde_json::json!({
                "id": profile_id,
                "inheritsFrom": minecraft,
                "mainClass": "net.minecraft.client.main.Main",
                "libraries": [{"name": coordinate}]
            });
            fs::write(
                profile_dir.join(format!("{profile_id}.json")),
                serde_json::to_vec(&profile).expect("loader metadata should encode"),
            )
            .expect("loader metadata should be written");
            fs::write(profile_dir.join(format!("{profile_id}.jar")), b"client")
                .expect("profile client jar should be written");
            let (found_id, _) = find_installed_pack_profile(&root, minecraft, Some(&loader))
                .expect("the exact pinned loader profile should be found");
            assert_eq!(found_id, profile_id);
        }
        fs::remove_dir_all(root).expect("temporary profile fixture should be removed");
    }

    #[test]
    fn rejects_multiple_and_unknown_modrinth_loader_dependencies() {
        let mut conflicting = format_v1_manifest(Some(("fabric-loader", "0.16.9")));
        conflicting
            .dependencies
            .insert("forge".to_string(), "47.4.29".to_string());
        assert!(validate_manifest(&conflicting)
            .unwrap_err()
            .contains("more than one mod loader"));

        let mut future_loader = format_v1_manifest(None);
        future_loader
            .dependencies
            .insert("future-loader".to_string(), "1.0.0".to_string());
        assert!(validate_manifest(&future_loader)
            .unwrap_err()
            .contains("unsupported future-loader dependency"));
    }

    #[test]
    fn builds_pinned_forge_and_neoforge_installer_urls() {
        let forge = LoaderPin {
            kind: LoaderKind::Forge,
            version: "47.4.29".to_string(),
        };
        assert_eq!(
            loader_installer_url("1.20.1", &forge).unwrap(),
            "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.1-47.4.29/forge-1.20.1-47.4.29-installer.jar"
        );
        let full_forge_pin = LoaderPin {
            kind: LoaderKind::Forge,
            version: "1.20.1-47.4.29".to_string(),
        };
        assert_eq!(
            loader_installer_url("1.20.1", &full_forge_pin).unwrap(),
            loader_installer_url("1.20.1", &forge).unwrap()
        );
        assert!(loader_installer_url("1.21.1", &full_forge_pin).is_err());

        let neoforge = LoaderPin {
            kind: LoaderKind::NeoForge,
            version: "21.1.266".to_string(),
        };
        assert_eq!(
            loader_installer_url("1.21.1", &neoforge).unwrap(),
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.266/neoforge-21.1.266-installer.jar"
        );
    }

    #[test]
    fn parses_and_validates_the_supplied_outbreak_pack() {
        let pack = Path::new("/workspace/OUTBREAK-Last-Signal-1.0.0.mrpack");
        if !pack.exists() {
            return;
        }
        let manifest = read_manifest(pack).expect("pack manifest should parse");
        let (minecraft, fabric) = validate_manifest(&manifest).expect("pack should be supported");
        assert_eq!(minecraft, "26.3");
        assert_eq!(
            fabric.as_ref().map(|loader| loader.version.as_str()),
            Some("0.19.5")
        );
        assert_eq!(manifest.files.len(), 21);
        assert!(manifest
            .files
            .iter()
            .all(|file| file.path.starts_with("mods/")));
    }

    #[test]
    fn extracts_and_verifies_all_mod_files_in_the_supplied_bee_zip() {
        let pack = Path::new("/workspace/OUTBREAK-Last-Signal-Bee-Launcher.zip");
        if !pack.exists() {
            return;
        }
        let manifest = read_manifest(pack).expect("Bee ZIP manifest should parse");
        let (minecraft, fabric) =
            validate_manifest(&manifest).expect("Bee ZIP manifest should be supported");
        assert_eq!(minecraft, "26.3");
        assert_eq!(
            fabric.as_ref().map(|loader| loader.version.as_str()),
            Some("0.19.5")
        );
        let entries = local_bundle_entries(pack, &manifest.files)
            .expect("bundle entries should match the manifest");
        assert_eq!(entries.len(), manifest.files.len());
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        let destination = std::env::temp_dir().join(format!("bee-bundle-test-{unique}"));
        fs::create_dir_all(&destination).expect("temporary test folder should be created");
        for pack_file in &manifest.files {
            let archive_name = entries
                .get(&pack_file.path)
                .expect("bundle should contain each manifest file");
            extract_bundle_file(pack, archive_name, &destination, pack_file)
                .unwrap_or_else(|error| panic!("{} should verify: {error}", pack_file.path));
        }
        assert_eq!(
            fs::read_dir(destination.join("mods"))
                .expect("mods folder should exist")
                .count(),
            manifest.files.len()
        );
        fs::remove_dir_all(destination).expect("temporary test folder should be removed");
    }

    #[test]
    fn prepares_outbreak_zip_from_installed_fabric_fixture_without_web_metadata() {
        let pack = Path::new("/workspace/OUTBREAK-Last-Signal-Bee-Launcher.zip");
        if !pack.exists() {
            return;
        }
        let manifest = read_manifest(pack).expect("Bee ZIP manifest should parse");
        let (minecraft, fabric) = validate_manifest(&manifest).expect("Bee ZIP manifest is valid");
        assert_eq!(minecraft, "26.3");
        assert_eq!(
            fabric.as_ref().map(|loader| loader.version.as_str()),
            Some("0.19.5")
        );

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bee-outbreak-offline-test-{unique}"));
        let source = root.join("installed-minecraft");
        let destination = root.join("instances/outbreak-last-signal");
        let profile_id = "Fabric 26.3";
        let profile_dir = source.join("versions").join(profile_id);
        fs::create_dir_all(&profile_dir).expect("fixture profile folder should be created");

        let client_jar = b"fixture minecraft client";
        let fabric_loader_jar = b"fixture Fabric loader";
        let library_jar = b"fixture minecraft library";
        let asset_bytes = b"fixture minecraft asset";
        let asset_hash = sha1_bytes(asset_bytes);
        let asset_index = serde_json::to_vec(&serde_json::json!({
            "objects": {
                "fixture/item": {
                    "hash": asset_hash,
                    "size": asset_bytes.len()
                }
            }
        }))
        .expect("fixture asset index should encode");

        let mut profile: Value =
            serde_json::from_str(include_str!("../tests/fixtures/Fabric26.3.json"))
                .expect("Fabric 26.3 fixture should parse");
        profile["libraries"] = serde_json::json!([
            {
                "name": "net.fabricmc:fabric-loader:0.19.5",
                "downloads": {
                    "artifact": {
                        "path": "net/fabricmc/fabric-loader/0.19.5/fabric-loader-0.19.5.jar",
                        "sha1": sha1_bytes(fabric_loader_jar),
                        "size": fabric_loader_jar.len(),
                        "url": "https://example.invalid/fabric-loader.jar"
                    }
                }
            },
            {
                "name": "test.offline:fixture-library:1.0",
                "downloads": {
                    "artifact": {
                        "path": "test/offline/fixture-library/1.0/fixture-library-1.0.jar",
                        "sha1": sha1_bytes(library_jar),
                        "size": library_jar.len(),
                        "url": "https://example.invalid/fixture-library.jar"
                    }
                }
            }
        ]);
        profile["downloads"]["client"] = serde_json::json!({
            "url": "https://example.invalid/client.jar",
            "sha1": sha1_bytes(client_jar),
            "size": client_jar.len()
        });
        profile["assetIndex"] = serde_json::json!({
            "id": "fixture-assets",
            "url": "https://example.invalid/fixture-assets.json",
            "size": asset_index.len()
        });

        fs::write(
            profile_dir.join(format!("{profile_id}.json")),
            serde_json::to_vec_pretty(&profile).expect("fixture profile should encode"),
        )
        .expect("fixture profile metadata should be written");
        fs::write(profile_dir.join(format!("{profile_id}.jar")), client_jar)
            .expect("fixture client JAR should be written");
        let loader_path =
            source.join("libraries/net/fabricmc/fabric-loader/0.19.5/fabric-loader-0.19.5.jar");
        fs::create_dir_all(
            loader_path
                .parent()
                .expect("Fabric library parent should exist"),
        )
        .expect("fixture Fabric library folder should be created");
        fs::write(&loader_path, fabric_loader_jar)
            .expect("fixture Fabric loader should be written");
        let library_path =
            source.join("libraries/test/offline/fixture-library/1.0/fixture-library-1.0.jar");
        fs::create_dir_all(library_path.parent().expect("library parent should exist"))
            .expect("fixture library folder should be created");
        fs::write(&library_path, library_jar).expect("fixture library JAR should be written");
        let index_path = source.join("assets/indexes/fixture-assets.json");
        fs::create_dir_all(index_path.parent().expect("index parent should exist"))
            .expect("fixture asset index folder should be created");
        fs::write(&index_path, &asset_index).expect("fixture asset index should be written");
        let asset_path = source.join(format!(
            "assets/objects/{}/{}",
            &asset_hash[..2],
            asset_hash
        ));
        fs::create_dir_all(asset_path.parent().expect("asset parent should exist"))
            .expect("fixture asset folder should be created");
        fs::write(&asset_path, asset_bytes).expect("fixture asset should be written");

        let (found_id, source_chain) =
            find_installed_pack_profile(&source, &minecraft, fabric.as_ref())
                .expect("installed Fabric profile should match the ZIP pins");
        assert_eq!(found_id, profile_id);
        copy_installed_profile(&source, &destination, &source_chain)
            .expect("installed profile metadata and client should be reused");
        let chain = super::super::metadata_chain(&destination, &found_id)
            .expect("copied profile chain should be valid");
        copy_installed_libraries(&source, &destination, &chain)
            .expect("installed libraries should be reused");
        copy_installed_assets(None, &source, &destination, &destination, &chain)
            .expect("installed assets should be reused");

        let entries = local_bundle_entries(pack, &manifest.files)
            .expect("bundle entries should match the manifest");
        assert_eq!(entries.len(), manifest.files.len());
        for pack_file in &manifest.files {
            let archive_name = entries
                .get(&pack_file.path)
                .expect("bundle should contain each manifest file");
            extract_bundle_file(pack, archive_name, &destination, pack_file)
                .unwrap_or_else(|error| panic!("{} should verify: {error}", pack_file.path));
        }
        assert_eq!(
            fs::read_dir(destination.join("mods"))
                .expect("instance mods folder should exist")
                .count(),
            21
        );
        assert_eq!(
            fs::read(destination.join(format!("versions/{profile_id}/{profile_id}.jar"))).unwrap(),
            client_jar
        );
        assert_eq!(
            fs::read(
                destination
                    .join("libraries/test/offline/fixture-library/1.0/fixture-library-1.0.jar")
            )
            .unwrap(),
            library_jar
        );
        assert_eq!(
            fs::read(destination.join(format!(
                "assets/objects/{}/{}",
                &asset_hash[..2],
                asset_hash
            )))
            .unwrap(),
            asset_bytes
        );
        assert!(source
            .join("versions")
            .join(profile_id)
            .join(format!("{profile_id}.json"))
            .is_file());
        fs::remove_dir_all(root).expect("fixture folders should be removed");
    }

    #[test]
    fn reuses_installed_minecraft_base_without_fetching_mojang_manifest() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bee-local-minecraft-reuse-{unique}"));
        let source = root.join("source");
        let destination = root.join("shared");
        let version_dir = source.join("versions/26.3");
        fs::create_dir_all(&version_dir).expect("installed version folder should be created");

        let client_jar = b"installed Minecraft 26.3 client";
        let asset_index = serde_json::to_vec(&serde_json::json!({"objects": {}}))
            .expect("asset index should encode");
        let metadata = serde_json::json!({
            "id": "26.3",
            "mainClass": "net.minecraft.client.main.Main",
            "downloads": {
                "client": {
                    "url": "https://example.invalid/client.jar",
                    "sha1": sha1_bytes(client_jar),
                    "size": client_jar.len()
                }
            },
            "assetIndex": {
                "id": "fixture-assets",
                "url": "https://example.invalid/assets.json",
                "sha1": sha1_bytes(&asset_index),
                "size": asset_index.len()
            },
            "libraries": []
        });
        fs::write(
            version_dir.join("26.3.json"),
            serde_json::to_vec_pretty(&metadata).expect("version metadata should encode"),
        )
        .expect("installed version metadata should be written");
        fs::write(version_dir.join("26.3.jar"), client_jar)
            .expect("installed client JAR should be written");
        let asset_index_path = source.join("assets/indexes/fixture-assets.json");
        fs::create_dir_all(
            asset_index_path
                .parent()
                .expect("asset index parent should exist"),
        )
        .expect("asset index folder should be created");
        fs::write(&asset_index_path, &asset_index)
            .expect("installed asset index should be written");

        let client = Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .expect("short-timeout client should build");
        let profile_id = install_version_metadata(
            None,
            &client,
            &destination,
            "26.3",
            None,
            Some(&source),
            &destination,
        )
        .expect("complete local Minecraft files should be reused without a web request");

        assert_eq!(profile_id, "26.3");
        assert_eq!(
            fs::read(destination.join("versions/26.3/26.3.jar")).unwrap(),
            client_jar
        );
        assert_eq!(
            super::super::metadata_at(&destination, "26.3").as_ref(),
            Some(&metadata)
        );
        assert_eq!(
            fs::read(destination.join("assets/indexes/fixture-assets.json")).unwrap(),
            asset_index
        );
        assert!(source.join("versions/26.3/26.3.jar").is_file());
        fs::remove_dir_all(root).expect("fixture folders should be removed");
    }

    #[test]
    fn uses_only_official_mojang_hosts_for_version_metadata_fallbacks() {
        let sources = mojang_version_metadata_sources(
            "https://piston-meta.mojang.com/v1/packages/hash/26.3.json",
        )
        .expect("pinned metadata should use an official Mojang host");
        assert_eq!(
            sources,
            [
                "https://piston-meta.mojang.com/v1/packages/hash/26.3.json",
                "https://launchermeta.mojang.com/v1/packages/hash/26.3.json"
            ]
        );
        assert!(mojang_version_metadata_sources("https://example.invalid/26.3.json").is_err());
        assert!(MINECRAFT_VERSION_MANIFEST_URLS
            .iter()
            .all(|url| url.starts_with("https://")));
    }

    #[test]
    #[ignore = "requires live Mojang and Fabric metadata services"]
    fn installs_minecraft_26_3_and_fabric_loader_0_19_5_in_a_separate_profile() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should be valid")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("bee-fabric-26-3-test-{unique}"));
        fs::create_dir_all(&directory).expect("temporary profile folder should be created");
        let client = super::super::downloader().expect("HTTP client should initialize");
        let loader = LoaderPin {
            kind: LoaderKind::Fabric,
            version: "0.19.5".to_string(),
        };
        let profile_id = install_version_metadata(
            None,
            &client,
            &directory,
            "26.3",
            Some(&loader),
            None,
            &directory,
        )
        .expect("Minecraft and Fabric metadata should install");
        assert_eq!(profile_id, "fabric-loader-0.19.5-26.3");
        let minecraft = fs::read_to_string(directory.join("versions/26.3/26.3.json"))
            .expect("Minecraft 26.3 metadata should be installed");
        let minecraft: Value = serde_json::from_str(&minecraft).expect("Minecraft JSON is valid");
        assert_eq!(minecraft.get("id").and_then(Value::as_str), Some("26.3"));
        let fabric_path = directory
            .join("versions")
            .join(&profile_id)
            .join(format!("{profile_id}.json"));
        let fabric: Value = serde_json::from_slice(
            &fs::read(fabric_path).expect("Fabric profile metadata should be installed"),
        )
        .expect("Fabric JSON is valid");
        assert_eq!(
            fabric.get("id").and_then(Value::as_str),
            Some(profile_id.as_str())
        );
        assert_eq!(
            fabric.get("inheritsFrom").and_then(Value::as_str),
            Some("26.3")
        );
        fs::remove_dir_all(directory).expect("temporary profile folder should be removed");
    }

    #[test]
    fn rejects_windows_and_parent_path_traversal_in_pack_files() {
        for path in [
            "../outside.jar",
            "mods/../../outside.jar",
            "C:/outside.jar",
            "mods\\outside.jar",
        ] {
            assert!(safe_pack_path(path).is_err(), "accepted unsafe path {path}");
        }
        assert!(safe_pack_path("mods/valid-mod.jar").is_ok());
    }
}
