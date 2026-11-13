#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::{
    env, fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{State, WebviewUrl, WebviewWindowBuilder};

const LAUNCHER_PAYLOAD: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/bee-launcher-payload.exe"));
const APP_ICON: &[u8] = include_bytes!("../icons/icon.ico");
const SHORTCUT_APP_ID_SCRIPT: &str =
    include_str!("../../../src-tauri/packaging/SetShortcutAppId.ps1");
const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
struct SetupState {
    uninstall_mode: bool,
    install_dir: PathBuf,
}

#[derive(Serialize)]
struct InstallerState {
    uninstall_mode: bool,
    install_dir: String,
    version: &'static str,
    already_installed: bool,
    release_payload_ready: bool,
}

#[derive(Serialize)]
struct InstallResult {
    launch_path: String,
    shortcut_warning: Option<String>,
}

struct InstallMutex(Mutex<()>);

#[tauri::command]
fn installer_state(state: State<'_, SetupState>) -> InstallerState {
    InstallerState {
        uninstall_mode: state.uninstall_mode,
        install_dir: state.install_dir.to_string_lossy().into_owned(),
        version: PRODUCT_VERSION,
        already_installed: state.install_dir.join("blockline-launcher.exe").is_file(),
        release_payload_ready: payload_is_pe(LAUNCHER_PAYLOAD),
    }
}

#[tauri::command]
fn install_launcher(
    state: State<'_, SetupState>,
    lock: State<'_, InstallMutex>,
    create_desktop_shortcut: bool,
) -> Result<InstallResult, String> {
    if state.uninstall_mode {
        return Err("This setup window is in uninstall mode.".to_string());
    }
    if !payload_is_pe(LAUNCHER_PAYLOAD) {
        return Err("This is a development setup build without an embedded Bee Launcher. Use the packaged Windows setup executable.".to_string());
    }
    let _guard = lock
        .0
        .lock()
        .map_err(|_| "Bee Launcher setup is already running.".to_string())?;
    let install_dir = &state.install_dir;
    validate_install_directory(install_dir)?;
    fs::create_dir_all(install_dir)
        .map_err(|error| format!("Could not create the Bee Launcher install folder: {error}"))?;

    let existing_launcher = install_dir.join("blockline-launcher.exe");
    let setup_executable = env::current_exe()
        .map_err(|error| format!("Could not locate the setup executable: {error}"))?;
    let setup_bytes = fs::read(&setup_executable).map_err(|error| {
        format!("Could not read the setup executable for the uninstaller: {error}")
    })?;
    if !payload_is_pe(&setup_bytes) {
        return Err("The setup executable is not a valid Windows executable.".to_string());
    }

    let staged_launcher = stage_file(install_dir, "blockline-launcher.exe", LAUNCHER_PAYLOAD)?;
    let staged_uninstaller = stage_file(install_dir, "uninstall.exe", &setup_bytes)?;
    let staged_icon = stage_file(install_dir, "icon.ico", APP_ICON)?;
    let mut replacements = Vec::new();
    for (staged, target) in [
        (staged_launcher, existing_launcher.clone()),
        (staged_uninstaller, install_dir.join("uninstall.exe")),
        (staged_icon, install_dir.join("icon.ico")),
    ] {
        match replace_with_backup(&staged, &target) {
            Ok(backup) => replacements.push((target, backup)),
            Err(error) => {
                rollback_replacements(&replacements);
                return Err(error);
            }
        }
    }

    let desktop_shortcut = if create_desktop_shortcut { "1" } else { "0" };
    if let Err(error) = register_shortcuts_and_uninstall(install_dir, desktop_shortcut) {
        rollback_replacements(&replacements);
        return Err(format!(
            "Bee Launcher files were not left installed because Windows could not finish creating its shortcuts and uninstall entry: {error}"
        ));
    }
    let shortcut_warning =
        set_shortcut_app_user_model_id(install_dir, create_desktop_shortcut).err();
    for (_, backup) in replacements {
        if let Some(backup) = backup {
            let _ = fs::remove_file(backup);
        }
    }
    Ok(InstallResult {
        launch_path: existing_launcher.to_string_lossy().into_owned(),
        shortcut_warning,
    })
}

#[tauri::command]
fn launch_installed_launcher(state: State<'_, SetupState>) -> Result<(), String> {
    if state.uninstall_mode {
        return Err("The launcher cannot be started from uninstall mode.".to_string());
    }
    let launcher = state.install_dir.join("blockline-launcher.exe");
    if !launcher.is_file() {
        return Err("Bee Launcher is not installed in the selected location.".to_string());
    }
    Command::new(&launcher)
        .current_dir(&state.install_dir)
        .spawn()
        .map_err(|error| format!("Bee Launcher could not be started: {error}"))?;
    Ok(())
}

#[tauri::command]
fn uninstall_launcher(app: tauri::AppHandle, state: State<'_, SetupState>) -> Result<(), String> {
    if !state.uninstall_mode {
        return Err("Uninstall mode was not requested.".to_string());
    }
    let current_exe = env::current_exe()
        .map_err(|error| format!("Could not locate the Bee uninstaller: {error}"))?;
    let install_dir = current_exe
        .parent()
        .ok_or_else(|| "The Bee uninstaller has no containing install folder.".to_string())?;
    if current_exe.file_name().and_then(|name| name.to_str()) != Some("uninstall.exe")
        || state.install_dir != install_dir
        || !install_dir.join("blockline-launcher.exe").is_file()
    {
        return Err(
            "This uninstaller did not run from a recognized Bee Launcher install folder."
                .to_string(),
        );
    }
    let script = uninstall_script();
    let script_path = temporary_script_path("bee-launcher-remove");
    fs::write(&script_path, script)
        .map_err(|error| format!("Could not prepare the Bee uninstaller: {error}"))?;
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .env("BEE_INSTALL_DIR", install_dir)
        .env("BEE_UNINSTALL_SCRIPT", &script_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .spawn()
        .map_err(|error| format!("Windows could not start the Bee cleanup step: {error}"))?;
    app.exit(0);
    Ok(())
}

fn main() {
    let uninstall_mode = env::args().any(|arg| arg == "--uninstall");
    let install_dir = if uninstall_mode {
        env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .unwrap_or_else(default_install_directory)
    } else {
        existing_install_directory().unwrap_or_else(default_install_directory)
    };
    let state = SetupState {
        uninstall_mode,
        install_dir,
    };

    tauri::Builder::default()
        .manage(state)
        .manage(InstallMutex(Mutex::new(())))
        .invoke_handler(tauri::generate_handler![
            installer_state,
            install_launcher,
            launch_installed_launcher,
            uninstall_launcher
        ])
        .setup(|app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Bee Launcher Setup")
                .inner_size(980.0, 650.0)
                .min_inner_size(900.0, 600.0)
                .decorations(false)
                .resizable(true)
                .center()
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Bee Launcher Setup failed to start");
}

fn default_install_directory() -> PathBuf {
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local_app_data).join("Bee Launcher");
    }
    env::temp_dir().join("Bee Launcher")
}

fn existing_install_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        use windows_sys::Win32::System::Registry::{
            RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ,
        };

        let key: Vec<u16> = "Software\\blockline\\Bee Launcher\0"
            .encode_utf16()
            .collect();
        let mut buffer = vec![0_u16; 32_768];
        let mut bytes = (buffer.len() * std::mem::size_of::<u16>()) as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status == 0 {
            let count = (bytes as usize / std::mem::size_of::<u16>()).min(buffer.len());
            let end = buffer[..count]
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(count);
            if end > 0 {
                let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..end]));
                if path.is_absolute()
                    && (path.join("blockline-launcher.exe").is_file()
                        || path.join("uninstall.exe").is_file())
                {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn payload_is_pe(bytes: &[u8]) -> bool {
    bytes.len() > 512 && bytes.starts_with(b"MZ")
}

fn validate_install_directory(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || path.parent().is_none() {
        return Err("Bee Launcher needs a valid absolute installation folder.".to_string());
    }
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            format!("Could not inspect the Bee Launcher install folder: {error}")
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(
                "The selected Bee Launcher install path is a link or is not a folder.".to_string(),
            );
        }
        let has_owned_app_file = ["blockline-launcher.exe", "uninstall.exe", "icon.ico"]
            .iter()
            .any(|name| path.join(name).is_file());
        let has_contents = fs::read_dir(path)
            .map_err(|error| format!("Could not inspect the Bee Launcher install folder: {error}"))?
            .next()
            .is_some();
        if has_contents && !has_owned_app_file {
            return Err("This folder contains files Bee Launcher does not own, so setup will not overwrite it. Choose or restore the standard Bee Launcher location.".to_string());
        }
    }
    Ok(())
}

fn stage_file(directory: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let staged = directory.join(format!(".{name}.bee-new-{}", std::process::id()));
    if staged.exists() {
        return Err(format!(
            "An earlier incomplete setup stage already exists at {}; it was left untouched.",
            staged.display()
        ));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(|error| format!("Could not stage {name}: {error}"))?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&staged);
        return Err(format!("Could not finish staging {name}: {error}"));
    }
    Ok(staged)
}

fn replace_with_backup(staged: &Path, target: &Path) -> Result<Option<PathBuf>, String> {
    let backup = target.with_file_name(format!(
        ".{}.bee-previous-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file"),
        std::process::id()
    ));
    if backup.exists() {
        return Err(format!(
            "A previous update backup already exists at {}; it was left untouched.",
            backup.display()
        ));
    }
    let had_target = target.exists();
    if had_target {
        fs::rename(target, &backup).map_err(|error| {
            format!(
                "Could not move the existing {} out of the way. Close Bee Launcher and try again: {error}",
                target.file_name().unwrap_or_default().to_string_lossy()
            )
        })?;
    }
    if let Err(error) = fs::rename(staged, target) {
        if had_target {
            let _ = fs::rename(&backup, target);
        }
        let _ = fs::remove_file(staged);
        return Err(format!("Could not install {}: {error}", target.display()));
    }
    Ok(had_target.then_some(backup))
}

fn rollback_replacements(replacements: &[(PathBuf, Option<PathBuf>)]) {
    for (target, backup) in replacements.iter().rev() {
        let _ = fs::remove_file(target);
        if let Some(backup) = backup {
            let _ = fs::rename(backup, target);
        }
    }
}

fn register_shortcuts_and_uninstall(
    install_dir: &Path,
    create_desktop_shortcut: &str,
) -> Result<(), String> {
    let script = r#"
$ErrorActionPreference = 'Stop'
$installDir = $env:BEE_INSTALL_DIR
$exe = Join-Path $installDir 'blockline-launcher.exe'
$icon = Join-Path $installDir 'icon.ico'
$programs = [Environment]::GetFolderPath('Programs')
$menuFolder = Join-Path $programs 'Bee Launcher'
New-Item -ItemType Directory -Path $menuFolder -Force | Out-Null
$shell = New-Object -ComObject WScript.Shell
function New-BeeShortcut([string]$path) {
    $link = $shell.CreateShortcut($path)
    $link.TargetPath = $exe
    $link.WorkingDirectory = $installDir
    $link.IconLocation = "$icon,0"
    $link.Description = 'Bee Launcher — Minecraft profiles, modpacks, and worlds'
    $link.Save()
}
New-BeeShortcut (Join-Path $menuFolder 'Bee Launcher.lnk')
if ($env:BEE_DESKTOP_SHORTCUT -eq '1') {
    New-BeeShortcut (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Bee Launcher.lnk')
}
$installKey = 'HKCU:\Software\blockline\Bee Launcher'
New-Item -Path $installKey -Force | Out-Null
Set-Item -Path $installKey -Value $installDir
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Bee Launcher'
New-Item -Path $uninstallKey -Force | Out-Null
$uninstaller = Join-Path $installDir 'uninstall.exe'
$uninstallString = '"' + $uninstaller + '" --uninstall'
Set-ItemProperty -Path $uninstallKey -Name DisplayName -Value 'Bee Launcher'
Set-ItemProperty -Path $uninstallKey -Name DisplayVersion -Value $env:BEE_VERSION
Set-ItemProperty -Path $uninstallKey -Name Publisher -Value 'Blockline'
Set-ItemProperty -Path $uninstallKey -Name InstallLocation -Value $installDir
Set-ItemProperty -Path $uninstallKey -Name DisplayIcon -Value $icon
Set-ItemProperty -Path $uninstallKey -Name UninstallString -Value $uninstallString
Set-ItemProperty -Path $uninstallKey -Name NoModify -Type DWord -Value 1
Set-ItemProperty -Path $uninstallKey -Name NoRepair -Type DWord -Value 1
"#;
    run_powershell(
        "bee-launcher-register",
        script,
        &[
            ("BEE_INSTALL_DIR", install_dir.to_string_lossy().as_ref()),
            ("BEE_DESKTOP_SHORTCUT", create_desktop_shortcut),
            ("BEE_VERSION", PRODUCT_VERSION),
        ],
        &[],
    )
}

fn set_shortcut_app_user_model_id(
    install_dir: &Path,
    create_desktop_shortcut: bool,
) -> Result<(), String> {
    let start_menu = env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| {
            "Windows did not provide the current user's roaming AppData folder.".to_string()
        })?
        .join("Microsoft/Windows/Start Menu/Programs/Bee Launcher/Bee Launcher.lnk");
    let mut shortcuts = vec![start_menu.to_string_lossy().into_owned()];
    if create_desktop_shortcut {
        let desktop = env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::temp_dir())
            .join("Desktop/Bee Launcher.lnk");
        shortcuts.push(desktop.to_string_lossy().into_owned());
    }
    let exe = install_dir.join("blockline-launcher.exe");
    let icon = install_dir.join("icon.ico");
    let args = [
        "-ExecutablePath".to_string(),
        exe.to_string_lossy().into_owned(),
        "-IconPath".to_string(),
        icon.to_string_lossy().into_owned(),
        "-AppUserModelId".to_string(),
        "com.blockline.launcher".to_string(),
        "-ShortcutPaths".to_string(),
        shortcuts.join("|"),
    ];
    run_powershell(
        "bee-launcher-shortcut-id",
        SHORTCUT_APP_ID_SCRIPT,
        &[],
        &args,
    )
}

fn run_powershell(
    stem: &str,
    script: &str,
    environment: &[(&str, &str)],
    args: &[String],
) -> Result<(), String> {
    let script_path = temporary_script_path(stem);
    fs::write(&script_path, script)
        .map_err(|error| format!("Could not prepare Windows setup actions: {error}"))?;
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in environment {
        command.env(name, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command
        .output()
        .map_err(|error| format!("Could not run Windows setup actions: {error}"))?;
    let _ = fs::remove_file(&script_path);
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if detail.is_empty() {
        format!("Windows setup actions exited with {}.", output.status)
    } else {
        detail
    })
}

fn temporary_script_path(stem: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    env::temp_dir().join(format!("{stem}-{}-{stamp}.ps1", std::process::id()))
}

fn uninstall_script() -> &'static str {
    r#"
$ErrorActionPreference = 'SilentlyContinue'
Start-Sleep -Seconds 2
$installDir = $env:BEE_INSTALL_DIR
$targetExe = Join-Path $installDir 'blockline-launcher.exe'
$shell = New-Object -ComObject WScript.Shell
function Remove-BeeLinkIfOwned([string]$path) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { return }
    try {
        $link = $shell.CreateShortcut($path)
        if ($link.TargetPath -and [String]::Equals([IO.Path]::GetFullPath($link.TargetPath), [IO.Path]::GetFullPath($targetExe), [StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $path -Force
        }
    } catch {}
}
$programs = [Environment]::GetFolderPath('Programs')
$desktop = [Environment]::GetFolderPath('Desktop')
$knownLinks = @(
    (Join-Path $programs 'Bee Launcher/Bee Launcher.lnk'),
    (Join-Path $desktop 'Bee Launcher.lnk')
)
foreach ($linkPath in $knownLinks) { Remove-BeeLinkIfOwned $linkPath }
$taskbarPins = Join-Path $env:APPDATA 'Microsoft/Internet Explorer/Quick Launch/User Pinned/TaskBar'
if (Test-Path -LiteralPath $taskbarPins -PathType Container) {
    Get-ChildItem -LiteralPath $taskbarPins -Filter '*.lnk' -File | ForEach-Object { Remove-BeeLinkIfOwned $_.FullName }
}
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Bee Launcher'
$installKey = 'HKCU:\Software\blockline\Bee Launcher'
Remove-Item -LiteralPath $uninstallKey -Recurse -Force
Remove-Item -LiteralPath $installKey -Recurse -Force
foreach ($name in @('blockline-launcher.exe','uninstall.exe','icon.ico','unins000.exe','unins000.dat')) {
    $file = Join-Path $installDir $name
    if (Test-Path -LiteralPath $file -PathType Leaf) { Remove-Item -LiteralPath $file -Force }
}
$menuFolder = Join-Path $programs 'Bee Launcher'
if (Test-Path -LiteralPath $menuFolder -PathType Container) {
    if ((Get-ChildItem -LiteralPath $menuFolder -Force | Measure-Object).Count -eq 0) { Remove-Item -LiteralPath $menuFolder -Force }
}
if (Test-Path -LiteralPath $installDir -PathType Container) {
    if ((Get-ChildItem -LiteralPath $installDir -Force | Measure-Object).Count -eq 0) { Remove-Item -LiteralPath $installDir -Force }
}
Remove-Item -LiteralPath $env:BEE_UNINSTALL_SCRIPT -Force
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_pe_or_too_small_launcher_payloads() {
        assert!(!payload_is_pe(b"MZ"));
        assert!(!payload_is_pe(&[0_u8; 1024]));
        let mut valid = vec![0_u8; 513];
        valid[..2].copy_from_slice(b"MZ");
        assert!(payload_is_pe(&valid));
    }

    #[test]
    fn installer_state_never_touches_game_or_user_data_paths() {
        let script = uninstall_script();
        assert!(script.contains("blockline-launcher.exe"));
        assert!(!script.contains("Remove-Item -LiteralPath $env:APPDATA"));
        assert!(!script.contains("minecraft-common"));
        assert!(!script.contains(".minecraft"));
        assert!(!script.contains("saves"));
    }
}
