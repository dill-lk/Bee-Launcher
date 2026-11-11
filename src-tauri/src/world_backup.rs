use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

const BACKUP_PREFIX: &str = ".bee-world-backup-";
const BACKUP_SUFFIX: &str = ".zip";
const MANIFEST_NAME: &str = "bee-world-backup.json";
const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 100_000;
const MAX_WORLDS: usize = 1_000;
const MAX_WORLD_NAME_BYTES: usize = 160;
const MAX_SOURCE_LABEL_BYTES: usize = 256;
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
static NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BackupSummary {
    pub id: String,
    pub created_at: u64,
    pub world_count: usize,
    pub world_names: Vec<String>,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RestoreWorldPreview {
    pub name: String,
    pub files: usize,
    pub bytes: u64,
    pub destination_exists: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RestorePreview {
    pub backup: BackupSummary,
    pub worlds: Vec<RestoreWorldPreview>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RestoreResult {
    pub restored_worlds: Vec<String>,
    pub safety_backup_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupManifest {
    version: u32,
    source_label: String,
    created_at: u64,
    world_names: Vec<String>,
}

#[derive(Debug)]
struct ArchiveData {
    manifest: BackupManifest,
    worlds: HashMap<String, Vec<ArchiveFile>>,
    total_bytes: u64,
}

#[derive(Debug, Clone)]
struct ArchiveFile {
    relative_path: PathBuf,
    bytes: Vec<u8>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn validate_world_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > MAX_WORLD_NAME_BYTES
        || name.trim() != name
        || name == "."
        || name == ".."
        || name.contains('\0')
        || !Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(format!("The world name is unsafe: {name}"));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() || path.contains('\0') || path.contains('\\') || path.starts_with('/') {
        return Err(format!("The archive contains an unsafe path: {path}"));
    }
    let path_buf = Path::new(path);
    if path_buf
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("The archive contains an unsafe path: {path}"));
    }
    Ok(path_buf.to_path_buf())
}

fn validate_backup_id(id: &str) -> Result<(), String> {
    if id.len() < BACKUP_PREFIX.len() + BACKUP_SUFFIX.len() + 3
        || !id.starts_with(BACKUP_PREFIX)
        || !id.ends_with(BACKUP_SUFFIX)
        || id.contains('/')
        || id.contains('\\')
        || !id[BACKUP_PREFIX.len()..id.len() - BACKUP_SUFFIX.len()]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase() || byte == b'-')
    {
        return Err("That backup ID is invalid.".to_string());
    }
    Ok(())
}

fn backup_path(backup_dir: &Path, backup_id: &str) -> Result<PathBuf, String> {
    validate_backup_id(backup_id)?;
    Ok(backup_dir.join(backup_id))
}

fn validate_manifest(manifest: &BackupManifest) -> Result<(), String> {
    if manifest.version != 1 {
        return Err("This world backup uses an unsupported format version.".to_string());
    }
    if manifest.source_label.trim().is_empty()
        || manifest.source_label.len() > MAX_SOURCE_LABEL_BYTES
    {
        return Err("The world backup source label is invalid.".to_string());
    }
    if manifest.world_names.is_empty() || manifest.world_names.len() > MAX_WORLDS {
        return Err("The world backup has an invalid world count.".to_string());
    }
    let mut names = HashSet::new();
    for name in &manifest.world_names {
        validate_world_name(name)?;
        if !names.insert(name) {
            return Err("The world backup lists a world more than once.".to_string());
        }
    }
    Ok(())
}

fn generated_id() -> String {
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed) ^ std::process::id() as u64;
    format!("{BACKUP_PREFIX}{}-{nonce:x}{BACKUP_SUFFIX}", now())
}

fn unique_path(dir: &Path) -> PathBuf {
    let id = generated_id();
    dir.join(id)
}

fn collect_files(
    root: &Path,
    current: &Path,
    output: &mut Vec<(PathBuf, Vec<u8>)>,
    total: &mut u64,
) -> Result<(), String> {
    let entries =
        fs::read_dir(current).map_err(|error| format!("Could not read world folder: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("Could not read world entry: {error}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("Could not inspect world entry: {error}"))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_files(root, &path, output, total)?;
        } else if metadata.is_file() {
            if output.len() >= MAX_FILES {
                return Err("The world contains too many files to back up safely.".to_string());
            }
            let size = metadata.len();
            if size > MAX_FILE_BYTES || total.saturating_add(size) > MAX_ARCHIVE_BYTES {
                return Err("The world backup is too large to create safely.".to_string());
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "Could not determine a world file path.".to_string())?
                .to_path_buf();
            let bytes =
                fs::read(&path).map_err(|error| format!("Could not read world file: {error}"))?;
            if bytes.len() as u64 != size {
                return Err("A world file changed while it was being backed up.".to_string());
            }
            *total += size;
            output.push((relative, bytes));
        }
    }
    Ok(())
}

fn write_manifest(writer: &mut ZipWriter<File>, manifest: &BackupManifest) -> Result<(), String> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let bytes = serde_json::to_vec(manifest)
        .map_err(|error| format!("Could not encode backup manifest: {error}"))?;
    writer
        .start_file(MANIFEST_NAME, options)
        .map_err(|error| format!("Could not write backup manifest: {error}"))?;
    writer
        .write_all(&bytes)
        .map_err(|error| format!("Could not write backup manifest: {error}"))
}

fn read_archive(path: &Path) -> Result<ArchiveData, String> {
    let file = File::open(path).map_err(|error| format!("Could not open world backup: {error}"))?;
    if file
        .metadata()
        .map_err(|error| format!("Could not inspect world backup: {error}"))?
        .len()
        > MAX_ARCHIVE_BYTES
    {
        return Err("The world backup is too large to inspect safely.".to_string());
    }
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("Could not read world backup: {error}"))?;
    if archive.len() > MAX_FILES + 2 {
        return Err("The world backup contains too many entries.".to_string());
    }
    let mut manifest: Option<BackupManifest> = None;
    let mut worlds: HashMap<String, Vec<ArchiveFile>> = HashMap::new();
    let mut total_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not read backup entry: {error}"))?;
        let name = entry.name().to_string();
        if name == MANIFEST_NAME {
            if manifest.is_some() || entry.is_dir() || entry.size() > MAX_MANIFEST_BYTES {
                return Err("The world backup manifest is invalid.".to_string());
            }
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|error| format!("Could not read backup manifest: {error}"))?;
            manifest = Some(
                serde_json::from_slice(&bytes)
                    .map_err(|error| format!("Could not parse backup manifest: {error}"))?,
            );
            continue;
        }
        let relative = name
            .strip_prefix("worlds/")
            .ok_or_else(|| "The world backup contains an unexpected entry.".to_string())?;
        let mut parts = relative.splitn(2, '/');
        let world = parts.next().unwrap_or("");
        let file_name = parts.next().unwrap_or("");
        validate_world_name(world)?;
        if file_name.is_empty() || entry.is_dir() {
            continue;
        }
        let relative_path = validate_relative_path(file_name)?;
        if entry.size() > MAX_FILE_BYTES
            || total_bytes.saturating_add(entry.size()) > MAX_ARCHIVE_BYTES
        {
            return Err("The world backup contains files that are too large.".to_string());
        }
        let mut bytes = Vec::with_capacity(entry.size().min(1024 * 1024) as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Could not read backup file: {error}"))?;
        if bytes.len() as u64 != entry.size() {
            return Err("A backup file was truncated while being read.".to_string());
        }
        total_bytes += bytes.len() as u64;
        worlds
            .entry(world.to_string())
            .or_default()
            .push(ArchiveFile {
                relative_path,
                bytes,
            });
    }
    let manifest = manifest.ok_or_else(|| "The world backup has no manifest.".to_string())?;
    validate_manifest(&manifest)?;
    let declared: HashSet<&String> = manifest.world_names.iter().collect();
    if worlds.keys().any(|name| !declared.contains(name))
        || manifest
            .world_names
            .iter()
            .any(|name| !worlds.contains_key(name))
    {
        return Err("The world backup contents do not match its manifest.".to_string());
    }
    Ok(ArchiveData {
        manifest,
        worlds,
        total_bytes,
    })
}

pub(super) fn list_backups(backup_dir: &Path) -> Result<Vec<BackupSummary>, String> {
    if !backup_dir.exists() {
        return Ok(Vec::new());
    }
    let mut summaries = Vec::new();
    for entry in fs::read_dir(backup_dir)
        .map_err(|error| format!("Could not read backup folder: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("Could not read backup folder entry: {error}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry
            .file_type()
            .map_err(|error| format!("Could not inspect backup: {error}"))?
            .is_file()
            || validate_backup_id(&name).is_err()
        {
            continue;
        }
        let data = read_archive(&entry.path())?;
        summaries.push(BackupSummary {
            id: name,
            created_at: data.manifest.created_at,
            world_count: data.manifest.world_names.len(),
            world_names: data.manifest.world_names,
            total_bytes: data.total_bytes,
        });
    }
    summaries.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(summaries)
}

pub(super) fn create_backup(
    backup_dir: &Path,
    saves_dir: &Path,
    source_label: &str,
) -> Result<BackupSummary, String> {
    if source_label.trim().is_empty() || source_label.len() > MAX_SOURCE_LABEL_BYTES {
        return Err("The backup source label is invalid.".to_string());
    }
    fs::create_dir_all(backup_dir)
        .map_err(|error| format!("Could not create backup folder: {error}"))?;
    let mut worlds = Vec::new();
    if saves_dir.is_dir() {
        for entry in fs::read_dir(saves_dir)
            .map_err(|error| format!("Could not read saves folder: {error}"))?
        {
            let entry =
                entry.map_err(|error| format!("Could not read saves folder entry: {error}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("Could not inspect world folder: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                continue;
            }
            validate_world_name(&name)?;
            worlds.push((name, entry.path()));
        }
    }
    worlds.sort_by(|left, right| left.0.cmp(&right.0));
    if worlds.is_empty() {
        return Err("No Minecraft worlds were found in this saves folder.".to_string());
    }
    if worlds.len() > MAX_WORLDS {
        return Err("There are too many worlds to back up safely.".to_string());
    }
    let created_at = now();
    let world_names = worlds
        .iter()
        .map(|world| world.0.clone())
        .collect::<Vec<_>>();
    let manifest = BackupManifest {
        version: 1,
        source_label: source_label.to_string(),
        created_at,
        world_names: world_names.clone(),
    };
    validate_manifest(&manifest)?;
    let final_path = unique_path(backup_dir);
    let temporary_path = backup_dir.join(format!(
        ".{}.tmp",
        final_path.file_name().unwrap().to_string_lossy()
    ));
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .map_err(|error| format!("Could not create backup: {error}"))?;
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut total_bytes = 0_u64;
        for (world, path) in worlds {
            let mut files = Vec::new();
            collect_files(&path, &path, &mut files, &mut total_bytes)?;
            for (relative, bytes) in files {
                let zip_name = format!(
                    "worlds/{}/{}",
                    world,
                    relative.to_string_lossy().replace('\\', "/")
                );
                validate_relative_path(&zip_name)?;
                writer
                    .start_file(zip_name, options)
                    .map_err(|error| format!("Could not write backup: {error}"))?;
                writer
                    .write_all(&bytes)
                    .map_err(|error| format!("Could not write backup: {error}"))?;
            }
        }
        write_manifest(&mut writer, &manifest)?;
        writer
            .finish()
            .map_err(|error| format!("Could not finish backup: {error}"))?;
        fs::rename(&temporary_path, &final_path)
            .map_err(|error| format!("Could not publish backup: {error}"))?;
        Ok(BackupSummary {
            id: final_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            created_at,
            world_count: world_names.len(),
            world_names,
            total_bytes,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

pub(super) fn inspect_backup(
    backup_dir: &Path,
    backup_id: &str,
    saves_dir: &Path,
) -> Result<RestorePreview, String> {
    let data = read_archive(&backup_path(backup_dir, backup_id)?)?;
    let backup = BackupSummary {
        id: backup_id.to_string(),
        created_at: data.manifest.created_at,
        world_count: data.manifest.world_names.len(),
        world_names: data.manifest.world_names.clone(),
        total_bytes: data.total_bytes,
    };
    let worlds = data
        .manifest
        .world_names
        .iter()
        .map(|name| {
            let files = data.worlds.get(name).map_or(0, Vec::len);
            let bytes = data.worlds.get(name).map_or(0, |entries| {
                entries.iter().map(|entry| entry.bytes.len() as u64).sum()
            });
            RestoreWorldPreview {
                name: name.clone(),
                files,
                bytes,
                destination_exists: fs::symlink_metadata(saves_dir.join(name)).is_ok(),
            }
        })
        .collect();
    Ok(RestorePreview { backup, worlds })
}

fn write_safety_backup(
    backup_dir: &Path,
    saves_dir: &Path,
    worlds: &[String],
) -> Result<String, String> {
    let temporary = saves_dir.join(format!(".{}.safety", generated_id()));
    fs::create_dir(&temporary)
        .map_err(|error| format!("Could not prepare safety backup: {error}"))?;
    let result = (|| {
        for name in worlds {
            let source = saves_dir.join(name);
            let destination = temporary.join(name);
            copy_world_tree(&source, &destination)?;
        }
        create_backup(backup_dir, &temporary, "restore safety backup")
    })();
    let _ = fs::remove_dir_all(&temporary);
    result
        .map(|summary| summary.id)
        .map_err(|error| format!("Could not create safety backup: {error}"))
}

fn copy_world_tree(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("Could not inspect conflicting world: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("A conflicting world is not a regular directory.".to_string());
    }
    fs::create_dir_all(destination)
        .map_err(|error| format!("Could not stage safety backup: {error}"))?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("Could not read conflicting world: {error}"))?
    {
        let entry = entry.map_err(|error| format!("Could not read conflicting world: {error}"))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)
            .map_err(|error| format!("Could not inspect conflicting world: {error}"))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            copy_world_tree(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path)
                .map_err(|error| format!("Could not stage safety backup: {error}"))?;
        }
    }
    Ok(())
}

pub(super) fn restore_backup(
    backup_dir: &Path,
    backup_id: &str,
    saves_dir: &Path,
    selected_worlds: &[String],
) -> Result<RestoreResult, String> {
    let data = read_archive(&backup_path(backup_dir, backup_id)?)?;
    let requested = if selected_worlds.is_empty() {
        data.manifest.world_names.clone()
    } else {
        selected_worlds.to_vec()
    };
    let declared: HashSet<&String> = data.manifest.world_names.iter().collect();
    let mut unique = HashSet::new();
    for name in &requested {
        validate_world_name(name)?;
        if !declared.contains(name) {
            return Err(format!("The backup does not contain world: {name}"));
        }
        if !unique.insert(name) {
            return Err("A world was selected more than once.".to_string());
        }
    }
    fs::create_dir_all(saves_dir)
        .map_err(|error| format!("Could not create saves folder: {error}"))?;
    let stage = saves_dir.join(format!(".bee-restore-stage-{}", generated_id()));
    fs::create_dir(&stage)
        .map_err(|error| format!("Could not create restore staging folder: {error}"))?;
    let result = (|| {
        for name in &requested {
            for entry in data.worlds.get(name).into_iter().flatten() {
                let destination = stage.join(name).join(&entry.relative_path);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| format!("Could not stage restore: {error}"))?;
                }
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)
                    .map_err(|error| format!("Could not stage restore: {error}"))?;
                file.write_all(&entry.bytes)
                    .map_err(|error| format!("Could not stage restore: {error}"))?;
            }
        }
        let conflicts = requested
            .iter()
            .filter(|name| fs::symlink_metadata(saves_dir.join(name)).is_ok())
            .cloned()
            .collect::<Vec<_>>();
        let safety_backup_id = if conflicts.is_empty() {
            None
        } else {
            Some(write_safety_backup(backup_dir, saves_dir, &conflicts)?)
        };
        let aside = saves_dir.join(format!(".bee-restore-aside-{}", generated_id()));
        fs::create_dir(&aside).map_err(|error| format!("Could not prepare restore: {error}"))?;
        let mut moved = Vec::new();
        let mutation = (|| {
            for name in &conflicts {
                fs::rename(saves_dir.join(name), aside.join(name))
                    .map_err(|error| format!("Could not move existing world aside: {error}"))?;
                moved.push(name.clone());
            }
            for name in &requested {
                fs::rename(stage.join(name), saves_dir.join(name))
                    .map_err(|error| format!("Could not install restored world: {error}"))?;
            }
            Ok::<(), String>(())
        })();
        if let Err(error) = mutation {
            for name in &requested {
                let _ = fs::remove_dir_all(saves_dir.join(name));
            }
            for name in moved.iter().rev() {
                let _ = fs::rename(aside.join(name), saves_dir.join(name));
            }
            return Err(error);
        }
        let _ = fs::remove_dir_all(&aside);
        Ok(RestoreResult {
            restored_worlds: requested,
            safety_backup_id,
        })
    })();
    let _ = fs::remove_dir_all(&stage);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "bee-world-backup-test-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn creates_inspects_and_restores_world() {
        let root = temp_dir("roundtrip");
        let saves = root.join("saves");
        let backups = root.join("backups");
        fs::create_dir_all(saves.join("Alpha")).unwrap();
        fs::write(saves.join("Alpha").join("level.dat"), b"level").unwrap();
        let backup = create_backup(&backups, &saves, "test").unwrap();
        let preview = inspect_backup(&backups, &backup.id, &saves).unwrap();
        assert_eq!(preview.worlds[0].name, "Alpha");
        fs::remove_dir_all(saves.join("Alpha")).unwrap();
        let restored =
            restore_backup(&backups, &backup.id, &saves, &["Alpha".to_string()]).unwrap();
        assert_eq!(restored.restored_worlds, vec!["Alpha"]);
        assert_eq!(
            fs::read(saves.join("Alpha").join("level.dat")).unwrap(),
            b"level"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unsafe_backup_id() {
        assert!(backup_path(Path::new("/tmp"), "../anything.zip").is_err());
        assert!(backup_path(Path::new("/tmp"), "not-a-backup.zip").is_err());
    }
}
