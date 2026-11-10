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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstanceMarker {
    instance_id: String,
    name: String,
    minecraft_version: String,
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
    fabric_loader: Option<String>,
    files_installed: usize,
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

fn validate_manifest(manifest: &ModrinthManifest) -> Result<(String, Option<String>), String> {
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
    for unsupported in ["quilt-loader", "forge", "neoforge"] {
        if manifest.dependencies.contains_key(unsupported) {
            return Err(format!(
                "This pack uses {unsupported}; this Bee Launcher build currently imports Fabric or vanilla Modrinth packs."
            ));
        }
    }
    let fabric = manifest.dependencies.get("fabric-loader").cloned();
    if let Some(loader) = fabric.as_deref() {
        validate_dependency_version(loader, "Fabric Loader")?;
    }
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
    Ok((minecraft.clone(), fabric))
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

fn install_version_metadata(
    app: Option<&AppHandle>,
    client: &Client,
    game_dir: &Path,
    minecraft: &str,
    fabric_loader: Option<&str>,
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
    let minecraft_metadata = request_json_from_sources(
        client,
        &minecraft_sources,
        &format!("Minecraft {minecraft} metadata"),
        expected_sha1,
        16 * 1024 * 1024,
    )?;
    if minecraft_metadata.get("id").and_then(Value::as_str) != Some(minecraft) {
        return Err("Mojang returned metadata for a different Minecraft version.".to_string());
    }
    let minecraft_path = safe_join(game_dir, &format!("versions/{minecraft}/{minecraft}.json"))?;
    write_json(
        &minecraft_path,
        &minecraft_metadata,
        "Minecraft version metadata",
    )?;

    let Some(loader) = fabric_loader else {
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
        return Ok(minecraft.to_string());
    };
    if let Some(app) = app {
        report_progress(
            app,
            "fabric",
            format!("Resolving Fabric Loader {loader}"),
            0,
            1,
            19,
        );
    }
    let url = format!(
        "https://meta.fabricmc.net/v2/versions/loader/{}/{}/profile/json",
        encode_path_segment(minecraft),
        encode_path_segment(loader)
    );
    let mut fabric_metadata = request_json(
        client,
        &url,
        &format!("Fabric Loader {loader} profile"),
        None,
        8 * 1024 * 1024,
    )?;
    let profile_id = fabric_metadata
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Fabric did not return a version profile ID.".to_string())?
        .to_string();
    ensure_version_id(&profile_id)?;
    if fabric_metadata.get("inheritsFrom").and_then(Value::as_str) != Some(minecraft) {
        return Err(
            "The Fabric profile does not inherit the modpack's pinned Minecraft version."
                .to_string(),
        );
    }
    if fabric_metadata
        .get("mainClass")
        .and_then(Value::as_str)
        .is_none()
    {
        return Err("Fabric returned a profile without a Java main class.".to_string());
    }
    if let Some(profile_id_field) = fabric_metadata
        .as_object_mut()
        .and_then(|object| object.get_mut("id"))
    {
        *profile_id_field = Value::String(profile_id.clone());
    }
    let fabric_path = safe_join(
        game_dir,
        &format!("versions/{profile_id}/{profile_id}.json"),
    )?;
    write_json(&fabric_path, &fabric_metadata, "Fabric Loader profile")?;
    if let Some(app) = app {
        report_progress(
            app,
            "fabric",
            format!("Fabric Loader {loader} profile ready"),
            1,
            1,
            22,
        );
    }
    Ok(profile_id)
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
    game_dir: &Path,
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
    let index_path = ensure_downloaded(client, game_dir, &spec, "Minecraft asset index")?;
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
            let game_dir = game_dir.to_path_buf();
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
    let is_bundle = extension.eq_ignore_ascii_case("zip");
    if !is_bundle && !extension.eq_ignore_ascii_case("mrpack") {
        return Err("Choose a Modrinth .mrpack file or a Bee Launcher bundle ZIP.".to_string());
    }
    report_progress(app, "validate", "Reading modpack manifest", 0, 1, 2);
    let manifest = read_manifest(&pack_path)?;
    let (minecraft_version, fabric_loader) = validate_manifest(&manifest)?;
    let local_entries = local_bundle_entries(&pack_path, &manifest.files)?;
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
        return Err("This ZIP does not contain every client mod listed in its Modrinth manifest. Choose the Bee Launcher bundle ZIP or the original .mrpack instead.".to_string());
    }
    let instances = app_instances_dir(app)?;
    let (instance_id, instance_dir) = allocate_instance_dir(&instances, &manifest.name)?;
    let result = (|| {
        report_progress(
            app,
            "validate",
            format!("Preparing {} as a separate instance", manifest.name),
            1,
            1,
            6,
        );
        copy_overrides(&pack_path, &instance_dir, app)?;
        let client = downloader()?;
        let profile_id = install_version_metadata(
            Some(app),
            &client,
            &instance_dir,
            &minecraft_version,
            fabric_loader.as_deref(),
        )?;
        report_progress(
            app,
            "minecraft",
            "Preparing Minecraft client and libraries",
            0,
            1,
            25,
        );
        let chain = super::metadata_chain(&instance_dir, &profile_id)?;
        super::ensure_client_jar(&client, &instance_dir, &chain)?;
        super::ensure_library_artifacts(&client, &instance_dir, &chain)?;
        let _ = super::install_bundled_authlib(&instance_dir, &chain)?;
        report_progress(
            app,
            "minecraft",
            "Minecraft client and libraries are ready",
            1,
            1,
            50,
        );
        install_assets(app, &client, &instance_dir, &chain)?;
        let installed_files = install_pack_files(
            app,
            &client,
            &instance_dir,
            &manifest.files,
            &pack_path,
            &local_entries,
            is_bundle,
        )?;
        let marker = InstanceMarker {
            instance_id: instance_id.clone(),
            name: manifest.name.clone(),
            minecraft_version: minecraft_version.clone(),
            fabric_loader: fabric_loader.clone(),
            profile_id: profile_id.clone(),
            pack_version: manifest.version_id.clone(),
        };
        let marker_path = instance_dir.join(".bee-instance.json");
        let marker_bytes = serde_json::to_vec_pretty(&marker)
            .map_err(|error| format!("Could not save modpack instance details: {error}"))?;
        fs::write(&marker_path, marker_bytes)
            .map_err(|error| format!("Could not finish installing the modpack: {error}"))?;
        report_progress(
            app,
            "complete",
            format!("{} installed successfully", manifest.name),
            installed_files,
            installed_files,
            100,
        );
        Ok(ImportResult {
            instance_name: manifest.name.clone(),
            profile_key: format!("pack:{instance_id}:{profile_id}"),
            minecraft_version: minecraft_version.clone(),
            fabric_loader: fabric_loader.clone(),
            files_installed: installed_files,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&instance_dir);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_validates_the_supplied_outbreak_pack() {
        let pack = Path::new("/workspace/OUTBREAK-Last-Signal-1.0.0.mrpack");
        if !pack.exists() {
            return;
        }
        let manifest = read_manifest(pack).expect("pack manifest should parse");
        let (minecraft, fabric) = validate_manifest(&manifest).expect("pack should be supported");
        assert_eq!(minecraft, "26.3");
        assert_eq!(fabric.as_deref(), Some("0.19.5"));
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
        assert_eq!(fabric.as_deref(), Some("0.19.5"));
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
        let profile_id =
            install_version_metadata(None, &client, &directory, "26.3", Some("0.19.5"))
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
