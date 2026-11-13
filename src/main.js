import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { defaultWindowIcon } from '@tauri-apps/api/app';
import { LogicalSize } from '@tauri-apps/api/dpi';
import { open } from '@tauri-apps/plugin-dialog';
import { getCurrentWindow } from '@tauri-apps/api/window';
import minecraftLogo from './assets/minecraft-logo.svg';
import trailsWallpaper from './assets/ui-trails-tales.jpg';
import bridgeWallpaper from './assets/ui-golem-bridge.jpg';
import sunsetWallpaper from './assets/ui-sunset-bridge.jpg';
import originalWallpaper from './assets/worldscape.png';
import { initializeSkinPreview, setSkinPreview } from './skinPreview.js';
import './style.css';

const $ = (selector) => document.querySelector(selector);
let launcherWindow = null;
try { launcherWindow = getCurrentWindow(); } catch { /* The launcher runs in Tauri; browser previews may not have the window API. */ }
const launcherWindowSizeKey = 'blockline.launcherWindowSize';
const versionSelect = $('#version-select');
const toast = $('#toast');
const sheet = $('#sheet');
let designSeed = 7;
function designRandom() {
  designSeed = (designSeed * 16807) % 2147483647;
  return designSeed / 2147483647;
}

function createNetherDuskWallpaper() {
  let stars = '';
  for (let index = 0; index < 46; index += 1) {
    const x = Math.floor(designRandom() * 160);
    const y = Math.floor(designRandom() * 48);
    const opacity = (0.3 + designRandom() * 0.6).toFixed(2);
    stars += `<rect x="${x}" y="${y}" width="1" height="1" fill="#fff" opacity="${opacity}"/>`;
  }
  const scene = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 90" shape-rendering="crispEdges"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#14082c"/><stop offset=".55" stop-color="#7a1f55"/><stop offset="1" stop-color="#ff8a3d"/></linearGradient></defs><rect width="160" height="90" fill="url(#g)"/>${stars}<rect x="116" y="30" width="9" height="9" fill="#ffe9b0"/><path d="M0 90V70h12v-6h10v8h14v-12h12v10h16v-8h14v14h18v-10h14v-12h12v16h18v20z" fill="#12061f"/></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(scene)}`;
}

const netherDuskWallpaper = createNetherDuskWallpaper();
const wallpaperOptions = [
  { id: 'trails', name: 'Trails & Tales', src: trailsWallpaper, showLogo: false, position: '50% 30%' },
  { id: 'bridge', name: 'Golem Bridge', src: bridgeWallpaper, showLogo: true, position: '50% 55%' },
  { id: 'nether', name: 'Nether Dusk', src: netherDuskWallpaper, showLogo: true, position: '50% 50%' },
  { id: 'sunset', name: 'Sunset Bridge', src: sunsetWallpaper, showLogo: true, position: '50% 50%' },
  { id: 'original', name: 'World Landscape', src: originalWallpaper, showLogo: true, position: '50% 50%' },
];
const accentColors = {
  green: ['#1f9d46', '#52e07f', '#0a5524', '#28b455'],
  blue: ['#2b7de0', '#6fb4ff', '#123f80', '#3a90f0'],
  orange: ['#e0821f', '#ffb85c', '#7a3f08', '#f0962f'],
  purple: ['#8a4be0', '#c196ff', '#46207a', '#9b5cf0'],
  red: ['#d93a3a', '#ff8a8a', '#701818', '#ee4a4a'],
};
const appearanceDefaults = { wallpaper: 'sunset', accent: 'green', opacity: 50, brightness: 100, blur: true };
const beeReleaseNotes = [
  {
    version: '0.1.33', date: '2026-11-13', title: 'Copy local Minecraft base on first import',
    changes: [
      'On the first modpack import from a selected .minecraft folder, physically copies its assets, libraries, and version metadata/JARs into Bee’s shared store; later profiles reuse that Bee-owned copy.',
      'Copies only immutable game-file folders. The selected source, mods, configs, saves, and other profile data stay untouched and isolated; conflicting shared files are preserved rather than overwritten.',
      'The Windows installer and uninstaller now use Bee-themed artwork, matching icons, and a friendly farewell header.',
    ],
  },
  {
    version: '0.1.32', date: '2026-11-13', title: 'CurseForge packs and shared game files',
    changes: [
      'Imports CurseForge ZIP exports directly, including the supplied Satisfaction Guaranteed pack, and resolves its pinned files from CurseForge.',
      'Shares common Minecraft assets, libraries, and version files across imported profiles while keeping mods, configs, resource packs, shaders, saves, and launch options profile-local.',
      'Existing imported profiles are migrated only when common files match; conflicting files stay in their original profile. The selected .minecraft folder is copied from, never linked or rewritten.',
    ],
  },
  {
    version: '0.1.31', date: '2026-11-12', title: 'Forge, NeoForge, and Quilt modpack imports',
    changes: [
      'Imports Modrinth format-v1 vanilla, Fabric, Quilt, Forge, and NeoForge packs as isolated profiles; Forge and NeoForge use their official client installers inside the new pack folder.',
      'Bee ZIP imports remain offline and reuse a matching Minecraft/loader profile already installed locally. CurseForge ZIP imports resolve the exact project/file IDs over HTTPS and support one pinned loader.',
    ],
  },
  {
    version: '0.1.30', date: '2026-11-12', title: 'Explicit Windows proxy handling for News',
    changes: [
      'Uses Windows WinHTTP to resolve the current user’s PAC/WPAD or static proxy and bypass rules for each official Minecraft News URL.',
      'Keeps the official page/RSS fallback, bounded request timeouts, manual refresh, and saved offline articles.',
    ],
  },
  {
    version: '0.1.29', date: '2026-11-12', title: 'More reliable official Minecraft News',
    changes: [
      'Loads article cards from Minecraft.net’s official News page first, with its RSS feed as a fallback.',
      'Correctly decodes the live RSS feed’s UTF-16 payload and Atom article links.',
    ],
  },
  {
    version: '0.1.28', date: '2026-11-12', title: 'Reliable official Minecraft News',
    changes: [
      'Loads news from Minecraft.net’s first-party RSS feed instead of scraping the page layout.',
      'Uses Windows system Internet settings for News requests and keeps manual refresh, official article links, and saved offline articles.',
    ],
  },
  {
    version: '0.1.27', date: '2026-11-12', title: 'Windows proxy-aware News requests',
    changes: [
      'Enabled Windows system proxy and bypass settings for official Minecraft News requests.',
    ],
  },
  {
    version: '0.1.26', date: '2026-11-12', title: 'Larger skin preview and resilient news',
    changes: [
      'Made the Play-screen 3D skin character larger without changing the viewer layout or saved skins.',
      'Improved the official News request and kept a separate last-successful-news cache for offline viewing.',
    ],
  },
  {
    version: '0.1.25', date: '2026-11-12', title: 'Readable, scrollable windows',
    changes: [
      'Made Settings, Mods, Skins, Installations, News, and Patch Notes scroll cleanly without clipping long text or controls.',
      'Set the desktop launcher interface to an 80% default scale and improved narrow-window text wrapping.',
    ],
  },
  {
    version: '0.1.24', date: '2026-11-11', title: 'Performance, safety, and news',
    changes: [
      'Redesigned Settings and added per-profile Java, memory, and graphics choices.',
      'Added an optional Intel UHD low-impact preset; it is off by default and restores the saved game graphics options when switched back to default.',
      'Added local world backups with a restore preview, explicit confirmation, and a safety copy before replacing same-named worlds.',
      'Added plain-English explanations for the most recent local launch log.',
      'Preloads the saved active skin before the 3D character is shown; skin application now distinguishes staging from a confirmed in-game load.',
      'Added on-demand official Minecraft News and a Bee Launcher release-notes page.',
    ],
  },
  {
    version: '0.1.23', date: 'Previous release', title: 'Custom skins and profile preview',
    changes: [
      'Added a saved skin library and interactive 3D character preview.',
      'Added local custom-skin staging for supported Fabric profiles with CustomSkinLoader.',
      'Added launcher appearance settings and isolated modpack profiles.',
    ],
  },
];
const avatarStorageKey = 'blockline.playerAvatar';
let appearance = loadAppearance();
let appStatus = { gameDirectory: '', installedVersions: [], javaAvailable: false };
let settings = { maxMemoryGb: 4, width: 1280, height: 720, fullscreen: false, wallpaperPath: null, wallpaperName: null };
let profileLaunchSettings = {};
let profileLaunchDrafts = {};
let profileLaunchSelectedKey = '';
let worldRestoreReview = null;
let customSkins = [];
let lastAppliedSkin = null;
let activeSkinId = null;
let customSkinSupport = { supported: false, reason: 'Select a Fabric 26.3 profile to check CustomSkinLoader.', loaderVersion: null, checked: false };
let toastTimer;
let busy = false;
let selectedVersion = '';
let activePanel = 'play';
let currentArt = $('#a1');
let nextArt = $('#a2');

function loadAppearance() {
  try {
    return { ...appearanceDefaults, ...JSON.parse(localStorage.getItem('blockline.appearance') || '{}') };
  } catch {
    return { ...appearanceDefaults };
  }
}

function saveAppearance() {
  try { localStorage.setItem('blockline.appearance', JSON.stringify(appearance)); } catch { /* storage may be unavailable */ }
}

function notify(message, duration = 4200) {
  toast.textContent = message;
  toast.classList.add('on');
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => toast.classList.remove('on'), duration);
}

function bindWindowControls() {
  const dragRegion = $('#window-drag');
  if (!launcherWindow) {
    document.body.classList.add('browser-preview');
    return;
  }
  const maximizeButton = $('#window-maximize');
  const syncMaximizeButton = async () => {
    const maximized = await launcherWindow.isMaximized();
    maximizeButton.setAttribute('aria-label', maximized ? 'Restore window' : 'Maximize window');
    maximizeButton.setAttribute('aria-pressed', String(maximized));
    maximizeButton.dataset.maximized = String(maximized);
    maximizeButton.title = maximized ? 'Restore' : 'Maximize';
  };
  $('#window-minimize').addEventListener('click', () => launcherWindow.minimize().catch((error) => notify(`Could not minimize the window: ${error}`)));
  maximizeButton.addEventListener('click', async () => {
    try {
      await launcherWindow.toggleMaximize();
      await syncMaximizeButton();
    } catch (error) {
      notify(`Could not resize the window: ${error}`);
    }
  });
  $('#window-close').addEventListener('click', () => launcherWindow.close().catch((error) => notify(`Could not close the window: ${error}`)));
  dragRegion.addEventListener('pointerdown', async (event) => {
    if (event.button !== 0 || event.detail > 1) return;
    try {
      if (!await launcherWindow.isMaximized()) await launcherWindow.startDragging();
    } catch { /* Ignore pointer cancellation while the window is closing. */ }
  });
  dragRegion.addEventListener('dblclick', async () => {
    try {
      await launcherWindow.toggleMaximize();
      await syncMaximizeButton();
    } catch (error) {
      notify(`Could not resize the window: ${error}`);
    }
  });
  document.querySelectorAll('[data-resize]').forEach((handle) => handle.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    launcherWindow.startResizeDragging(handle.dataset.resize).catch(() => {});
  }));
  syncMaximizeButton().catch(() => {});
}

async function restoreWindowSize() {
  if (!launcherWindow) return;
  let savedSize = null;
  try {
    savedSize = JSON.parse(localStorage.getItem(launcherWindowSizeKey) || 'null');
  } catch { /* Ignore invalid or unavailable browser storage. */ }

  let resizeTimer;
  let pendingSize = null;
  try {
    await launcherWindow.onResized(async ({ payload }) => {
      try {
        if (await launcherWindow.isMaximized()) return;
        const scale = await launcherWindow.scaleFactor();
        const width = Math.round(payload.width / scale);
        const height = Math.round(payload.height / scale);
        if (!Number.isInteger(width) || width < 960 || width > 7680 || !Number.isInteger(height) || height < 540 || height > 4320) return;
        pendingSize = { width, height };
        window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(() => {
          if (!pendingSize) return;
          try { localStorage.setItem(launcherWindowSizeKey, JSON.stringify(pendingSize)); } catch { /* Storage may be unavailable. */ }
        }, 250);
      } catch { /* Ignore size events while the window is closing. */ }
    });
    window.addEventListener('beforeunload', () => {
      window.clearTimeout(resizeTimer);
      if (pendingSize) {
        try { localStorage.setItem(launcherWindowSizeKey, JSON.stringify(pendingSize)); } catch { /* Storage may be unavailable. */ }
      }
    });
    if (Number.isInteger(savedSize?.width) && savedSize.width >= 960 && savedSize.width <= 7680
      && Number.isInteger(savedSize?.height) && savedSize.height >= 540 && savedSize.height <= 4320) {
      await launcherWindow.setSize(new LogicalSize(savedSize.width, savedSize.height));
    }
  } catch (error) {
    console.warn('Could not restore the Bee Launcher window size:', error);
  }
}

async function applyDefaultWindowIcon() {
  if (!launcherWindow) return;
  try {
    const icon = await defaultWindowIcon();
    if (icon) await launcherWindow.setIcon(icon);
  } catch (error) {
    console.warn('Could not set the Bee Launcher window icon:', error);
  }
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char]);
}

function friendlyPath(path) {
  if (!path) return 'No Minecraft folder selected';
  const parts = path.replaceAll('\\', '/').split('/').filter(Boolean);
  return parts.slice(-5).join(' / ') || path;
}

function playerName() {
  return (localStorage.getItem('blockline.playerName') || 'Player').trim();
}

function validatePlayerName(name) {
  return /^[A-Za-z0-9_]{3,16}$/.test(name);
}

function displayKind(kind) {
  const label = String(kind || 'Profile').replaceAll('_', ' ').trim();
  return label ? `${label[0].toUpperCase()}${label.slice(1).toLowerCase()}` : 'Profile';
}

const avatarSkin = ['Explorer', '#c68a5f', '#4a2f1b', '#3b6fd6'];
const avatarPattern = ['hhhhhhhh', 'hhhhhhhh', 'hsssssss', 'sessssse', 'ssssssss', 'sssmmsss', 'ssssssss', 'ssssssss'];

function renderAvatar() {
  const activeSkin = customSkins.find((skin) => skin.id === activeSkinId);
  const avatar = activeSkin?.faceDataUrl || localStorage.getItem(avatarStorageKey);
  document.querySelectorAll('#me, #avatar-preview').forEach((face) => {
    face.replaceChildren();
    if (avatar) {
      face.classList.add('dp');
      face.style.setProperty('--dpi', `url("${avatar}")`);
      return;
    }
    face.classList.remove('dp');
    face.style.removeProperty('--dpi');
    avatarPattern.forEach((row) => {
      [...row].forEach((token) => {
        const cell = document.createElement('i');
        cell.style.background = token === 'h' ? avatarSkin[2] : token === 'e' ? avatarSkin[3] : token === 'm' ? '#8a4a3a' : avatarSkin[1];
        face.appendChild(cell);
      });
    });
  });
}

function createPixelIcon(top, body, gradient) {
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 16;
  const context = canvas.getContext('2d');
  for (let y = 0; y < 16; y += 1) {
    for (let x = 0; x < 16; x += 1) {
      const colors = gradient && (y < 4 || (y < 6 && designRandom() < 0.4)) ? top : body;
      context.fillStyle = colors[Math.floor(designRandom() * colors.length)];
      context.fillRect(x, y, 1, 1);
    }
  }
  return canvas.toDataURL();
}

function initializeReferenceArtwork() {
  $('#minecraft-logo').src = minecraftLogo;
  const grass = ['#5da83a', '#4f9530', '#69b845', '#4a8a2c'];
  const dirt = ['#866043', '#79553a', '#6b4a2f', '#966c4b'];
  const redstone = ['#b5522a', '#a3471f', '#c25f33', '#8f3a18'];
  const lapis = ['#4fb8ff', '#2f8de0', '#6fd0ff', '#1d6fd0'];
  const icons = [
    createPixelIcon(grass, dirt, true),
    createPixelIcon(null, dirt, false),
    createPixelIcon(null, redstone, false),
    createPixelIcon(null, lapis, false),
    createPixelIcon(grass, dirt, true),
  ];
  $('#i5').src = icons[4];
  renderAvatar();
}

function updateProfileName(name) {
  $('#account-name').textContent = name || 'Player';
  localStorage.setItem('blockline.playerName', name || 'Player');
  renderAvatar();
}

function updateNav(tab) {
  document.querySelectorAll('#sub button[data-s]').forEach((button) => {
    if (button.dataset.s === tab) button.setAttribute('aria-current', 'page');
    else button.removeAttribute('aria-current');
  });
}

function profileKeyOf(profile) {
  return profile.profileKey || `minecraft:${profile.id}`;
}

function profileJavaReady(profile) {
  return profile.javaAvailable ?? appStatus.javaAvailable;
}

function renderVersions() {
  const versions = appStatus.installedVersions || [];
  if (!versions.length) {
    versionSelect.innerHTML = '<option value="">No version profiles detected</option>';
    selectedVersion = '';
    $('#play-button').disabled = true;
    $('#quick-play').disabled = true;
    $('#vn').textContent = 'No profiles found';
    $('#vv').textContent = 'Check the .minecraft folder';
    $('#stat').textContent = appStatus.gameDirectory ? 'No version JSON files found in this folder.' : 'Choose the populated .minecraft folder in Settings.';
    $('#status-text').textContent = appStatus.gameDirectory ? 'Folder found but no installed profiles were detected' : 'Select a Minecraft folder';
    return;
  }

  versionSelect.innerHTML = versions.map((version) => {
    const label = version.instanceName ? `${version.instanceName} · ${version.id}` : version.id;
    return `<option value="${escapeHtml(profileKeyOf(version))}">${escapeHtml(label)} · ${escapeHtml(version.kind)}</option>`;
  }).join('');
  if (!versions.some((version) => profileKeyOf(version) === selectedVersion)) {
    const preferred = versions.find((version) => version.launchable && profileJavaReady(version)) || versions.find((version) => version.launchable) || versions[0];
    selectedVersion = profileKeyOf(preferred);
  }
  versionSelect.value = selectedVersion;
  const selected = versions.find((version) => profileKeyOf(version) === selectedVersion);
  const javaReady = selected ? profileJavaReady(selected) : false;
  const canLaunch = Boolean(selected?.launchable) && javaReady;
  $('#play-button').disabled = busy || !canLaunch;
  $('#quick-play').disabled = busy || !canLaunch;
  $('#vn').textContent = selected ? (selected.instanceName || selected.id) : 'No profile selected';
  const kindLabel = displayKind(selected?.kind);
  const javaLabel = selected?.requiredJavaMajor ? `Java ${selected.requiredJavaMajor}` : 'Java';
  $('#vv').textContent = selected ? `${kindLabel}${selected.instanceName ? ' · Imported pack' : ''} · ${selected.launchable ? (javaReady ? 'Ready' : `${javaLabel} needed`) : 'Files incomplete'}` : 'Local profile';
  $('#stat').textContent = selected
    ? (selected.launchable ? (javaReady ? 'Ready to launch this installed profile.' : `Profile found, but a compatible ${javaLabel} runtime was not found.`) : 'Profile detected, but one or more required local files are missing.')
    : 'Choose an installed profile.';
  $('#status-text').textContent = selected
    ? `${kindLabel} · ${selected.launchable ? (javaReady ? 'Ready to play' : `${javaLabel} not found`) : 'Profile detected · files incomplete'}`
    : 'No profile selected';
}

function updateSettingsPath() {
  const path = $('#settings-path');
  if (path) path.textContent = friendlyPath(appStatus.gameDirectory);
  const count = $('#profile-count');
  if (count) count.textContent = String(appStatus.installedVersions?.length || 0);
  const javaStatus = $('#java-status');
  if (javaStatus) javaStatus.textContent = appStatus.javaAvailable ? 'Java is ready' : 'Java was not detected';
  const javaDot = $('#java-dot');
  if (javaDot) javaDot.classList.toggle('ready', Boolean(appStatus.javaAvailable));
}

function renderInstallations() {
  if (activePanel !== 'inst') return;
  const list = $('#installations-list');
  if (!list) return;
  const versions = appStatus.installedVersions || [];
  if (!versions.length) {
    list.innerHTML = '<div class="item"><span class="dot" style="background:#ffcf4a"></span><div><b>No profiles detected</b><small>Choose the populated .minecraft folder, then rescan it.</small></div></div>';
  } else {
    list.innerHTML = versions.map((version) => {
      const javaReady = profileJavaReady(version);
      const ready = version.launchable && javaReady;
      const javaLabel = version.requiredJavaMajor ? `Java ${version.requiredJavaMajor}` : 'Java';
      const detail = [version.instanceName ? 'Imported modpack' : displayKind(version.kind), version.instanceName ? displayKind(version.kind) : '', version.baseVersion ? `Minecraft ${version.baseVersion}` : '', version.launchable ? (javaReady ? 'Ready' : `${javaLabel} needed`) : 'Detected · incomplete'].filter(Boolean).join(' · ');
      const label = version.instanceName ? `${version.instanceName} · ${version.id}` : version.id;
      return `<div class="item"><span class="dot" style="background:${ready ? '#34e070' : '#ffcf4a'}"></span><div class="profile-item-copy"><b>${escapeHtml(label)}</b><small>${escapeHtml(detail)}</small></div><button class="go" data-run-profile="${escapeHtml(profileKeyOf(version))}" ${ready ? '' : 'disabled'}>${version.launchable ? (javaReady ? 'Play' : 'Java needed') : 'Not ready'}</button></div>`;
    }).join('');
  }
  list.querySelectorAll('[data-run-profile]').forEach((button) => button.addEventListener('click', () => launchVersion(button.dataset.runProfile)));
}

function renderImportedPacks() {
  if (activePanel !== 'mods') return;
  const list = $('#modpack-list');
  if (!list) return;
  const profiles = (appStatus.installedVersions || []).filter((profile) => profile.instanceName);
  if (!profiles.length) {
    list.innerHTML = '<div class="item"><span class="dot" style="background:#ffcf4a"></span><div><b>No imported packs yet</b><small>Choose a Modrinth .mrpack, CurseForge ZIP, or Bee Launcher ZIP to install one.</small></div></div>';
    return;
  }
  list.innerHTML = profiles.map((profile) => {
    const javaReady = profileJavaReady(profile);
    const ready = profile.launchable && javaReady;
    const javaLabel = profile.requiredJavaMajor ? `Java ${profile.requiredJavaMajor}` : 'Java';
    const details = [`Minecraft ${profile.baseVersion || 'version metadata'}`, displayKind(profile.kind), ready ? 'Ready to play' : (profile.launchable ? `${javaLabel} needed` : 'Files incomplete')].join(' · ');
    return `<div class="item"><span class="dot" style="background:${ready ? '#34e070' : '#ffcf4a'}"></span><div class="profile-item-copy"><b>${escapeHtml(profile.instanceName)}</b><small>${escapeHtml(details)}</small></div><button class="go" data-select-profile="${escapeHtml(profileKeyOf(profile))}">Select</button></div>`;
  }).join('');
  list.querySelectorAll('[data-select-profile]').forEach((button) => button.addEventListener('click', () => {
    selectedVersion = button.dataset.selectProfile;
    openPanel('play');
    renderVersions();
    notify('Profile selected.', 2600);
  }));
}

function renderStatus() {
  renderVersions();
  renderInstallations();
  renderImportedPacks();
  updateSettingsPath();
}

async function refreshStatus(showMessage = false) {
  try {
    appStatus = await invoke('app_status');
    renderStatus();
    if (activePanel === 'skins') refreshCustomSkinSupport();
    if (activePanel === 'settings') renderProfileLaunchControls();
    if (activePanel === 'mods') renderWorldBackupPanel();
    if (showMessage) {
      const ready = appStatus.installedVersions.filter((version) => version.launchable).length;
      notify(`${appStatus.installedVersions.length} profile${appStatus.installedVersions.length === 1 ? '' : 's'} detected · ${ready} ready to launch.`);
    }
  } catch (error) {
    notify(`Could not scan your Minecraft folder: ${error}`, 7000);
  }
}

function reflectSettings(prefs) {
  if (prefs.profileSettings) profileLaunchSettings = structuredClone(prefs.profileSettings);
  settings = { ...settings, ...prefs };
  updateSettingsPath();
  const memory = $('#memory-range');
  if (memory) memory.value = settings.maxMemoryGb || 4;
  const memoryValue = $('#memory-value');
  if (memoryValue) memoryValue.textContent = `${settings.maxMemoryGb || 4} GB`;
  const opacityValue = $('#glass-opacity-value');
  if (opacityValue) opacityValue.textContent = `${appearance.opacity}%`;
  const brightnessValue = $('#wallpaper-brightness-value');
  if (brightnessValue) brightnessValue.textContent = `${appearance.brightness}%`;
  const width = $('#resolution-width');
  if (width) width.value = settings.width || 1280;
  const height = $('#resolution-height');
  if (height) height.value = settings.height || 720;
  const fullscreen = $('#fullscreen');
  if (fullscreen) fullscreen.checked = Boolean(settings.fullscreen);
  const wallpaperName = $('#wallpaper-name');
  if (wallpaperName) wallpaperName.textContent = settings.wallpaperName || (settings.wallpaperPath ? settings.wallpaperPath.split(/[\\/]/).pop() : 'Built-in design wallpaper');
  updateWallpaperSelection();
}

function renderProfileLaunchControls() {
  const select = $('#profile-launch-select');
  if (!select) return;
  const versions = appStatus.installedVersions || [];
  if (!versions.length) {
    select.innerHTML = '<option value="">No installed profiles detected</option>';
    select.disabled = true;
    ['#profile-memory', '#profile-graphics', '#profile-java-choose', '#profile-java-reset'].forEach((selector) => { const control = $(selector); if (control) control.disabled = true; });
    $('#profile-java-value').textContent = 'Choose a Minecraft profile first';
    $('#profile-preset-note').textContent = 'Scan your Minecraft folder to configure profile-specific performance.';
    return;
  }
  select.disabled = false;
  ['#profile-memory', '#profile-graphics', '#profile-java-choose', '#profile-java-reset'].forEach((selector) => { const control = $(selector); if (control) control.disabled = false; });
  select.innerHTML = versions.map((version) => {
    const key = profileKeyOf(version);
    const label = version.instanceName ? `${version.instanceName} · ${version.id}` : version.id;
    return `<option value="${escapeHtml(key)}">${escapeHtml(label)} · ${escapeHtml(version.kind)}</option>`;
  }).join('');
  if (!versions.some((version) => profileKeyOf(version) === profileLaunchSelectedKey)) {
    profileLaunchSelectedKey = versions.some((version) => profileKeyOf(version) === selectedVersion)
      ? selectedVersion
      : profileKeyOf(versions[0]);
  }
  select.value = profileLaunchSelectedKey;
  const profile = versions.find((version) => profileKeyOf(version) === profileLaunchSelectedKey);
  const prefs = profileLaunchDrafts[profileLaunchSelectedKey] || profileLaunchSettings[profileLaunchSelectedKey] || {};
  $('#profile-memory').value = prefs.maxMemoryGb ?? '';
  $('#profile-graphics').value = prefs.graphicsPreset || 'default';
  const javaValue = $('#profile-java-value');
  if (prefs.javaPath) {
    javaValue.textContent = prefs.javaPath;
    javaValue.title = prefs.javaPath;
    $('#profile-java-hint').textContent = 'Custom runtime · checked against this profile before launch.';
  } else {
    javaValue.textContent = 'Automatic detection';
    javaValue.title = 'Uses a compatible Java runtime already installed on this PC.';
    $('#profile-java-hint').textContent = profile?.requiredJavaMajor ? `Automatically picks Java ${profile.requiredJavaMajor} for this profile.` : 'Automatically picks a compatible Java runtime.';
  }
  $('#profile-preset-note').textContent = `${profile?.kind || 'Profile'} · ${profile?.requiredJavaMajor ? `requires Java ${profile.requiredJavaMajor}` : 'Java version detected automatically'} · ${prefs.maxMemoryGb ? `${prefs.maxMemoryGb} GB override` : `uses ${settings.maxMemoryGb || 4} GB default memory`}.`;
}

function updateProfileLaunchDraft(field, value) {
  if (!profileLaunchSelectedKey) return;
  const current = profileLaunchDrafts[profileLaunchSelectedKey] || profileLaunchSettings[profileLaunchSelectedKey] || {};
  profileLaunchDrafts[profileLaunchSelectedKey] = {
    javaPath: current.javaPath ?? null,
    maxMemoryGb: current.maxMemoryGb ?? null,
    graphicsPreset: current.graphicsPreset || 'default',
    [field]: value,
  };
  renderProfileLaunchControls();
}

async function pickProfileJava() {
  try {
    const selected = await open({ directory: false, multiple: false, title: 'Choose a Java executable for this profile', filters: [{ name: 'Java executable', extensions: ['exe'] }] });
    if (selected) updateProfileLaunchDraft('javaPath', selected);
  } catch (error) {
    notify(`Could not select that Java runtime: ${error}`, 7000);
  }
}

async function explainLastLaunch() {
  const key = $('#profile-launch-select')?.value || selectedVersion;
  const output = $('#crash-diagnosis');
  const button = $('#diagnose-last-launch');
  if (!key || !output || !button) {
    notify('Select an installed profile first.');
    return;
  }
  output.hidden = false;
  output.textContent = 'Reading the selected profile’s local launch log…';
  button.disabled = true;
  try {
    const diagnosis = await invoke('diagnose_launch_log', { profileKey: key });
    output.innerHTML = `<div class="crash-diagnosis-heading"><b>${escapeHtml(diagnosis.title)}</b><p>${escapeHtml(diagnosis.summary)}</p></div><div class="crash-diagnosis-columns"><div><b>Possible causes</b><ul>${diagnosis.likelyCauses.map((item) => `<li>${escapeHtml(item)}</li>`).join('')}</ul></div><div><b>Try this</b><ul>${diagnosis.nextSteps.map((item) => `<li>${escapeHtml(item)}</li>`).join('')}</ul></div></div>${diagnosis.logExcerpt ? `<details><summary>Redacted log excerpt</summary><pre>${escapeHtml(diagnosis.logExcerpt)}</pre></details>` : ''}`;
  } catch (error) {
    output.textContent = `Could not analyze this profile's log: ${error}`;
  } finally {
    button.disabled = false;
  }
}

function swapWallpaper(src, showLogo = false, position = '50% 50%') {
  const image = new Image();
  image.onload = () => {
    nextArt.src = src;
    nextArt.style.objectPosition = position;
    nextArt.classList.add('on');
    currentArt.classList.remove('on');
    [currentArt, nextArt] = [nextArt, currentArt];
    $('#bd').src = src;
    $('#logo').classList.toggle('on', showLogo);
  };
  image.onerror = () => {
    nextArt.src = src;
    nextArt.style.objectPosition = position;
    nextArt.classList.add('on');
    currentArt.classList.remove('on');
    [currentArt, nextArt] = [nextArt, currentArt];
    $('#bd').src = src;
    $('#logo').classList.toggle('on', showLogo);
  };
  image.src = src;
}

function applyStockWallpaper(id = 'sunset') {
  const option = wallpaperOptions.find((item) => item.id === id) || wallpaperOptions[0];
  appearance.wallpaper = option.id;
  saveAppearance();
  swapWallpaper(option.src, option.showLogo, option.position);
  updateWallpaperSelection();
}

function setCustomWallpaper(dataUrl) {
  if (dataUrl) swapWallpaper(dataUrl, true, '50% 50%');
  updateWallpaperSelection();
}

function applyAppearance() {
  const root = document.documentElement.style;
  const color = accentColors[appearance.accent] || accentColors.green;
  root.setProperty('--green', color[0]);
  root.setProperty('--green-hi', color[1]);
  root.setProperty('--green-lo', color[2]);
  root.setProperty('--green-top', color[3]);
  root.setProperty('--ga', String((appearance.opacity || 50) / 100));
  root.setProperty('--panel-alpha', String((appearance.opacity || 50) / 100));
  root.setProperty('--br', String((appearance.brightness || 100) / 100));
  document.body.classList.toggle('nobd', !appearance.blur);
}

function updateWallpaperSelection() {
  document.querySelectorAll('[data-wallpaper]').forEach((button) => {
    button.setAttribute('aria-pressed', String(button.dataset.wallpaper === appearance.wallpaper && !settings.wallpaperPath));
  });
  const label = $('#wallpaper-name');
  if (label) label.textContent = settings.wallpaperPath ? (settings.wallpaperName || settings.wallpaperPath.split(/[\\/]/).pop()) : (wallpaperOptions.find((item) => item.id === appearance.wallpaper)?.name || 'Built-in design wallpaper');
}

function panelHeader(title, kicker = 'Bee Launcher') {
  return `<div class="sh"><div><small class="panel-kicker">${escapeHtml(kicker)}</small><h2>${escapeHtml(title)}</h2></div><button data-close-panel aria-label="Close panel">✕</button></div>`;
}

function openPanel(name, title = '') {
  activePanel = name;
  sheet.innerHTML = '';
  sheet.classList.remove('on', 'big', 'settings-panel');
  updateNav(name === 'settings' ? 'play' : name);
  if (name === 'play') return;
  sheet.classList.add('on');
  sheet.classList.toggle('big', name === 'settings' || name === 'inst' || name === 'mods' || name === 'skins' || name === 'news' || name === 'notes');
  sheet.classList.toggle('news-panel', name === 'news' || name === 'notes');
  sheet.classList.toggle('skin-panel', name === 'skins');
  if (name === 'settings') {
    sheet.classList.add('settings-panel');
    profileLaunchDrafts = structuredClone(profileLaunchSettings);
    profileLaunchSelectedKey = selectedVersion;
    sheet.innerHTML = settingsMarkup();
    renderAvatar();
    bindSettings();
    reflectSettings(settings);
    renderProfileLaunchControls();
  } else if (name === 'inst') {
    sheet.innerHTML = `${panelHeader('Installations', 'Local Minecraft profiles and imported packs')}<div class="panel-scroll"><div class="profile-list" id="installations-list"></div><div class="panel-actions"><button class="btn" id="rescan-button">Rescan</button><button class="btn" id="choose-folder-modal">Choose folder</button><button class="btn pri" id="import-modpack-from-inst">Import Modpack</button></div></div>`;
    renderInstallations();
    $('#rescan-button').addEventListener('click', () => refreshStatus(true));
    $('#choose-folder-modal').addEventListener('click', pickGameFolder);
    $('#import-modpack-from-inst').addEventListener('click', () => openPanel('mods'));
  } else if (name === 'mods') {
    sheet.innerHTML = `${panelHeader('Import a modpack', 'Modrinth .mrpack or CurseForge ZIP')}
      <div class="panel-scroll">
      <div class="pack-import-card"><div class="pack-import-copy"><b>Install as a separate profile</b><small>Each profile keeps its own mods, configs, resource packs, shaders, saves, and options. Common Minecraft assets, libraries, and version files are shared to save disk space. Your selected .minecraft folder stays untouched.</small></div><button class="btn pri" id="import-modpack-button">Choose modpack file</button>
        <div class="import-progress" id="modpack-progress" hidden><div class="import-progress-copy"><b id="modpack-progress-title">Getting ready…</b><small id="modpack-progress-message">Modrinth and CurseForge imports may download game files and loader components; Bee ZIPs reuse a matching local profile.</small></div><progress id="modpack-progress-bar" max="100" value="0"></progress><small id="modpack-progress-detail">0%</small></div>
      </div>
      <div class="section-intro modpack-list-heading"><h3>Imported profiles</h3><p>Select one here or choose it from the Play profile menu.</p></div><div class="profile-list" id="modpack-list"></div>
      <div class="local-note"><span class="note-mark">i</span><span>Modrinth format-v1 and CurseForge Minecraft modpack ZIPs support vanilla, Fabric, Quilt, Forge, or NeoForge with one loader per pack. CurseForge imports fetch exact pinned files over HTTPS; Bee ZIP bundles remain offline and need a matching profile plus every listed client file inside the ZIP. Mods, configs, resource packs, shaders, saves, and options stay profile-local; common game files are reused from Bee's shared store. Modrinth downloads are checked against manifest hashes; CurseForge downloads are checked against the pinned file IDs and file sizes.</span></div>
      <section class="world-backup-card"><div class="section-intro"><h3>World backups</h3><p>Create a local safety copy, or review a backup before restoring it. Imported packs keep separate saves.</p></div><div class="world-backup-controls"><label for="world-backup-profile">Saves folder</label><select class="fld" id="world-backup-profile"><option value="">Default Minecraft folder</option></select><button class="btn pri" id="create-world-backup">Back up worlds</button></div><div class="world-backup-status" id="world-backup-status" role="status"></div><div class="world-backup-list" id="world-backup-list"></div><div class="world-restore-review" id="world-restore-review" hidden></div></section>
      </div>`;
    $('#import-modpack-button').addEventListener('click', pickModpack);
    renderImportedPacks();
    $('#create-world-backup').addEventListener('click', createWorldBackup);
    renderWorldBackupPanel();
  } else if (name === 'news') {
    sheet.innerHTML = `${panelHeader('Minecraft News', 'Official announcements from Minecraft.net')}<div class="panel-scroll"><div class="news-page-toolbar"><p>News loads only when you open this page or choose Refresh.</p><button class="btn" id="refresh-minecraft-news">Refresh news</button></div><div class="minecraft-news-status" id="minecraft-news-status" role="status">Loading official news…</div><div class="minecraft-news-grid" id="minecraft-news-grid"></div></div>`;
    $('#refresh-minecraft-news').addEventListener('click', loadMinecraftNews);
    loadMinecraftNews();
  } else if (name === 'notes') {
    sheet.innerHTML = `${panelHeader('Patch Notes', 'Bee Launcher releases')}<div class="panel-scroll"><div class="patch-notes-intro"><b>Local release history</b><p>These notes describe changes to Bee Launcher. Minecraft game snapshots and release notes are published on the official News page.</p></div><div class="patch-notes-list">${renderPatchNotes()}</div></div>`;
  } else if (name === 'skins') {
    sheet.innerHTML = `${panelHeader('Custom skins', 'Local skins for your Bee profile')}
      <div class="panel-scroll">
      <div class="skin-heading"><div class="section-intro"><h3>Your skin library</h3><p>Add a Minecraft skin PNG for the launcher avatar, or apply it in-game when CustomSkinLoader is installed.</p></div><button class="btn pri" id="skin-add">Add skin PNG</button></div>
      <div class="skin-support" id="skin-support-status" role="status" aria-live="polite"></div>
      <div class="custom-skin-grid" id="custom-skin-grid"></div>
      <section class="skin-application-card" id="skin-application-card" hidden></section>
      <div class="local-note"><span class="note-mark">i</span><span>In-game skins use CustomSkinLoader's LocalSkin folder and work only on this client; other players will not see them. On first apply, Bee moves LocalSkin ahead of online skin sources because the default order can ignore the local file. Existing config is backed up before changes; if it does not exist yet, Bee stages a one-use ExtraList entry. Bee checks the copied PNG and source order, but the loader log is checked separately after you fully restart Minecraft and open a world. Bee does not sign in, upload skins, install/update mods, or change worlds, profile JSON, or authentication. Same-name LocalSkin images are backed up before replacement; removing a library skin does not delete an already-applied file.</span></div>
      </div>`;
    renderCustomSkins();
    refreshCustomSkinSupport();
    $('#skin-add').addEventListener('click', addCustomSkin);
  } else {
    const messages = {
      realms: 'Realms is not available in this local launcher. It only starts profiles already installed on this device.',
      servers: 'This local launcher does not browse or join servers. Online play requiring Microsoft or TLauncher authentication is unsupported.',
    };
    sheet.innerHTML = `${panelHeader(title || 'Unavailable')}<div class="panel-scroll"><div class="item"><span class="dot" style="background:#ffcf4a"></span><div><b>Not part of this local build</b><small>${escapeHtml(messages[name] || 'This feature is not available in this local launcher.')}</small></div></div></div>`;
  }
  const close = sheet.querySelector('[data-close-panel]');
  if (close) close.addEventListener('click', () => openPanel('play'));
}

function renderPatchNotes() {
  return beeReleaseNotes.map((release, index) => `<article class="patch-note-card"><div class="patch-note-meta"><span>Bee Launcher</span><b>v${escapeHtml(release.version)}</b><time>${escapeHtml(release.date)}</time></div><h3>${escapeHtml(release.title)}</h3><ul>${release.changes.map((change) => `<li>${escapeHtml(change)}</li>`).join('')}</ul>${index === 0 ? '<span class="patch-note-current">CURRENT RELEASE</span>' : ''}</article>`).join('');
}

async function loadMinecraftNews() {
  const status = $('#minecraft-news-status');
  const grid = $('#minecraft-news-grid');
  const button = $('#refresh-minecraft-news');
  if (!status || !grid || !button) return;
  button.disabled = true;
  button.textContent = 'Refreshing…';
  status.textContent = 'Fetching announcements from the official Minecraft website…';
  grid.innerHTML = '';
  try {
    const result = await invoke('fetch_minecraft_news');
    const articles = result.articles;
    grid.innerHTML = articles.map((article) => `<article class="minecraft-news-card"><span class="news-card-source">MINECRAFT.NET · OFFICIAL</span><h3>${escapeHtml(article.title)}</h3><p>${escapeHtml(article.summary || 'Read the latest announcement from Mojang.')}</p><button class="btn news-read-link" data-news-url="${escapeHtml(article.url)}">Read article</button></article>`).join('');
    if (result.cached) {
      const savedAt = new Date(result.fetchedAt * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });
      status.textContent = `Live refresh failed; showing saved official news from ${savedAt}. ${result.error || ''}`;
    } else {
      status.textContent = `Updated ${new Date(result.fetchedAt * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}. News is fetched only when requested.`;
    }
    grid.querySelectorAll('[data-news-url]').forEach((link) => link.addEventListener('click', async () => {
      try { await invoke('open_minecraft_news_link', { url: link.dataset.newsUrl }); }
      catch (error) { notify(`Could not open the official article: ${error}`, 7000); }
    }));
  } catch (error) {
    status.textContent = `Could not refresh official news: ${error}`;
    grid.innerHTML = '<article class="minecraft-news-card news-fallback"><span class="news-card-source">OFFICIAL SOURCE</span><h3>Visit Minecraft.net</h3><p>Open the official News page in your browser to see the latest announcements.</p><button class="btn" id="open-minecraft-news-page">Open Minecraft News</button></article>';
    $('#open-minecraft-news-page').addEventListener('click', async () => {
      try { await invoke('open_minecraft_news_link', { url: 'https://www.minecraft.net/en-us/article' }); }
      catch (openError) { notify(`Could not open Minecraft.net: ${openError}`, 7000); }
    });
  } finally {
    button.disabled = false;
    button.textContent = 'Refresh news';
  }
}

function formatFileSize(bytes) {
  if (!Number.isFinite(bytes) || bytes < 1) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  let amount = bytes;
  let unit = 0;
  while (amount >= 1024 && unit < units.length - 1) { amount /= 1024; unit += 1; }
  return `${amount.toFixed(unit === 0 || amount >= 10 ? 0 : 1)} ${units[unit]}`;
}

async function renderWorldBackupPanel() {
  const select = $('#world-backup-profile');
  const list = $('#world-backup-list');
  if (!select || !list) return;
  const selected = select.value || selectedVersion;
  const profiles = appStatus.installedVersions || [];
  select.innerHTML = '<option value="">Default Minecraft folder</option>' + profiles.map((profile) => {
    const key = profileKeyOf(profile);
    const label = profile.instanceName ? `${profile.instanceName} · ${profile.id}` : profile.id;
    return `<option value="${escapeHtml(key)}">${escapeHtml(label)}</option>`;
  }).join('');
  select.value = profiles.some((profile) => profileKeyOf(profile) === selected) ? selected : '';
  try {
    const backups = await invoke('list_world_backups');
    if (!backups.length) {
      list.innerHTML = '<div class="world-backup-empty">No backups yet. Create one to safely preserve the selected saves folder.</div>';
      return;
    }
    list.innerHTML = backups.map((backup) => `<article class="world-backup-item"><div class="world-backup-item-main"><b>${backup.worldCount} world${backup.worldCount === 1 ? '' : 's'} · ${formatFileSize(backup.totalBytes)}</b><small>${escapeHtml(new Date(backup.createdAt * 1000).toLocaleString())}</small><span>${escapeHtml(backup.worldNames.join(' · '))}</span></div><button class="btn" data-review-backup="${escapeHtml(backup.id)}">Review restore</button></article>`).join('');
    list.querySelectorAll('[data-review-backup]').forEach((button) => button.addEventListener('click', () => openWorldRestoreReview(button.dataset.reviewBackup)));
  } catch (error) {
    list.innerHTML = `<div class="world-backup-empty error">Could not read local backups: ${escapeHtml(String(error))}</div>`;
  }
}

async function createWorldBackup() {
  const button = $('#create-world-backup');
  const status = $('#world-backup-status');
  if (!button || !status) return;
  button.disabled = true;
  status.textContent = 'Creating a local backup…';
  try {
    const summary = await invoke('create_world_backup', { profileKey: $('#world-backup-profile').value || null });
    status.textContent = `Backup created: ${summary.worldCount} world${summary.worldCount === 1 ? '' : 's'} · ${formatFileSize(summary.totalBytes)}.`;
    await renderWorldBackupPanel();
  } catch (error) {
    status.textContent = `Could not create a backup: ${error}`;
  } finally {
    button.disabled = false;
  }
}

async function openWorldRestoreReview(backupId) {
  const review = $('#world-restore-review');
  const select = $('#world-backup-profile');
  if (!review || !select) return;
  const profileKey = select.value || null;
  select.disabled = true;
  review.hidden = false;
  review.innerHTML = '<div class="world-restore-loading">Inspecting the backup and checking for same-named worlds…</div>';
  try {
    const preview = await invoke('inspect_world_backup', { backupId, profileKey });
    worldRestoreReview = { backupId, profileKey, preview };
    const conflicts = preview.worlds.filter((world) => world.destinationExists).length;
    review.innerHTML = `<div class="world-restore-heading"><div><b>Review restore</b><small>${escapeHtml(new Date(preview.backup.createdAt * 1000).toLocaleString())} · ${preview.worlds.length} world${preview.worlds.length === 1 ? '' : 's'}</small></div><button class="btn subtle" id="world-restore-cancel">Cancel</button></div><div class="world-restore-risk ${conflicts ? 'has-conflicts' : ''}">${conflicts ? `${conflicts} selected destination${conflicts === 1 ? '' : 's'} already exist and will be replaced. Bee will first create a separate safety backup of those existing worlds.` : 'No same-named worlds exist in this destination.'} Close Minecraft before restoring; selected worlds will be copied into this profile's saves folder.</div><div class="world-restore-worlds">${preview.worlds.map((world) => `<label class="world-restore-row"><input type="checkbox" class="world-restore-choice" value="${escapeHtml(world.name)}" checked><span><b>${escapeHtml(world.name)}</b><small>${world.files} files · ${formatFileSize(world.bytes)}${world.destinationExists ? ' · WILL REPLACE EXISTING WORLD' : ''}</small></span></label>`).join('')}</div><label class="world-restore-confirm-row"><input type="checkbox" id="world-restore-ack"><span>I reviewed the selected worlds and want to restore them.</span></label><div class="world-restore-actions"><button class="btn" id="world-restore-cancel-bottom">Cancel</button><button class="btn pri" id="world-restore-confirm" disabled>Restore selected worlds</button></div>`;
    const confirm = $('#world-restore-confirm');
    const acknowledgement = $('#world-restore-ack');
    const updateConfirm = () => { confirm.disabled = !acknowledgement.checked || !review.querySelector('.world-restore-choice:checked'); };
    acknowledgement.addEventListener('change', updateConfirm);
    review.querySelectorAll('.world-restore-choice').forEach((checkbox) => checkbox.addEventListener('change', updateConfirm));
    $('#world-restore-cancel').addEventListener('click', closeWorldRestoreReview);
    $('#world-restore-cancel-bottom').addEventListener('click', closeWorldRestoreReview);
    confirm.addEventListener('click', confirmWorldRestore);
  } catch (error) {
    worldRestoreReview = null;
    review.innerHTML = `<div class="world-restore-loading error">Could not inspect this backup: ${escapeHtml(String(error))}</div><button class="btn" id="world-restore-cancel">Close</button>`;
    $('#world-restore-cancel').addEventListener('click', closeWorldRestoreReview);
  }
}

function closeWorldRestoreReview() {
  worldRestoreReview = null;
  const review = $('#world-restore-review');
  if (review) { review.hidden = true; review.innerHTML = ''; }
  const select = $('#world-backup-profile');
  if (select) select.disabled = false;
}

async function confirmWorldRestore() {
  if (!worldRestoreReview) return;
  const review = $('#world-restore-review');
  const acknowledgement = $('#world-restore-ack');
  const selectedWorlds = [...review.querySelectorAll('.world-restore-choice:checked')].map((checkbox) => checkbox.value);
  if (!acknowledgement?.checked || selectedWorlds.length === 0) return;
  const button = $('#world-restore-confirm');
  button.disabled = true;
  button.textContent = 'Restoring…';
  try {
    const result = await invoke('restore_world_backup', {
      backupId: worldRestoreReview.backupId,
      profileKey: worldRestoreReview.profileKey,
      selectedWorlds,
    });
    const status = $('#world-backup-status');
    if (status) status.textContent = `Restored ${result.restoredWorlds.join(', ')}.${result.safetyBackupId ? ` Replaced worlds were saved first as ${result.safetyBackupId}.` : ''}`;
    closeWorldRestoreReview();
    await renderWorldBackupPanel();
  } catch (error) {
    button.disabled = false;
    button.textContent = 'Restore selected worlds';
    const status = $('#world-backup-status');
    if (status) status.textContent = `Restore stopped without replacing worlds: ${error}`;
  }
}

function settingsMarkup() {
  const options = wallpaperOptions.map((item) => `<button class="wallpaper-card" data-wallpaper="${item.id}" aria-pressed="false"><img src="${escapeHtml(item.src)}" alt=""><span>${escapeHtml(item.name)}</span></button>`).join('');
  const accents = Object.entries(accentColors).map(([key, color]) => `<button class="accent-swatch" data-accent="${key}" aria-label="${key} accent" title="${key[0].toUpperCase()}${key.slice(1)}" aria-pressed="${key === appearance.accent}" style="background:linear-gradient(${color[3]},${color[2]})"></button>`).join('');
  const profileMemoryOptions = `<option value="">Use launcher default (${settings.maxMemoryGb || 4} GB)</option>${Array.from({ length: 16 }, (_, index) => `<option value="${index + 1}">${index + 1} GB</option>`).join('')}`;
  return `${panelHeader('Settings', 'Make Bee Launcher yours')}
    <div class="settings-layout">
      <nav class="settings-rail" aria-label="Settings sections">
        <div class="settings-rail-heading"><small>LAUNCHER PREFERENCES</small><b>Setup workspace</b></div>
        <button data-settings-tab="profile" aria-controls="settings-profile" aria-current="page"><span class="rail-mark" aria-hidden="true">◉</span><span>Profile</span></button>
        <button data-settings-tab="game" aria-controls="settings-game"><span class="rail-mark" aria-hidden="true">▣</span><span>Game</span></button>
        <button data-settings-tab="appearance" aria-controls="settings-appearance"><span class="rail-mark" aria-hidden="true">✳</span><span>Appearance</span></button>
        <button data-settings-tab="folder" aria-controls="settings-folder-page"><span class="rail-mark" aria-hidden="true">▱</span><span>Minecraft folder</span></button>
        <button data-settings-tab="about" aria-controls="settings-about"><span class="rail-mark" aria-hidden="true">?</span><span>About</span></button>
        <div class="settings-rail-status"><span class="settings-local-dot"></span><div><b>Saved on this PC</b><small>Your launcher stays local</small></div></div>
      </nav>
      <div class="settings-pages">
        <section class="settings-page active" id="settings-profile" data-settings-page="profile">
          <div class="settings-page-heading"><div><small>PLAYER</small><h3>Your profile</h3><p>Set the name and picture used by this local launcher.</p></div><span class="settings-chip"><i></i> LOCAL PROFILE</span></div>
          <div class="setting-card profile-card">
            <div class="field-stack"><label for="settings-username">Player name</label><input class="fld" id="settings-username" maxlength="16" autocomplete="off" value="${escapeHtml(playerName())}" placeholder="Your Minecraft name"><small>3–16 letters, numbers, or underscores. Used for local offline play.</small><span class="field-local-hint"><i>✓</i> Stored only on this PC</span></div>
            <div class="avatar-card"><span class="face" id="avatar-preview" aria-label="Profile picture preview"></span><div class="avatar-copy"><b>Profile picture</b><small>Personalize the launcher avatar. It does not change your Minecraft skin.</small><div class="button-pair"><button class="btn" id="avatar-upload">Choose picture</button><button class="btn subtle" id="avatar-reset">Reset</button></div></div></div>
          </div>
          <div class="settings-callout"><span class="note-mark">i</span><span>Bee Launcher does not connect to Microsoft or TLauncher accounts, or read their sign-in tokens.</span></div>
        </section>
        <section class="settings-page" id="settings-game" data-settings-page="game" hidden>
          <div class="settings-page-heading"><div><small>LAUNCH BEHAVIOUR</small><h3>Game settings</h3><p>Choose the defaults used the next time you start Minecraft.</p></div></div>
          <div class="setting-card setting-list">
            <div class="setting-row"><div><label for="memory-range">Default memory limit</label><small>Used by profiles without their own memory override.</small></div><div class="range-control"><output id="memory-value">${settings.maxMemoryGb || 4} GB</output><input type="range" id="memory-range" min="1" max="16" step="1" value="${settings.maxMemoryGb || 4}" aria-label="Default maximum memory"></div></div>
            <div class="setting-row"><div><label for="resolution-width">Game window size</label><small>Choose the launch window dimensions in pixels.</small></div><div class="resolution-inputs"><input class="fld" type="number" min="800" max="7680" id="resolution-width" aria-label="Window width" value="${settings.width || 1280}"><span>×</span><input class="fld" type="number" min="600" max="4320" id="resolution-height" aria-label="Window height" value="${settings.height || 720}"></div></div>
            <div class="setting-row"><div><label for="fullscreen">Start in fullscreen</label><small>Open the game without a window frame.</small></div><label class="tg"><input type="checkbox" id="fullscreen" ${settings.fullscreen ? 'checked' : ''}><i></i></label></div>
          </div>
          <div class="setting-card profile-launch-card">
            <div class="profile-launch-heading"><div><h4>Profile-specific performance</h4><p>Choose a Java runtime, memory limit, and graphics preset for each installed profile.</p></div><span class="settings-chip">PER PROFILE</span></div>
            <div class="field-stack profile-select-stack"><label for="profile-launch-select">Installed profile</label><select class="fld" id="profile-launch-select" aria-label="Profile to configure"><option value="">Loading profiles…</option></select></div>
            <div class="profile-launch-grid">
              <div class="profile-setting"><label for="profile-memory">Memory for this profile</label><small>Overrides the default memory above.</small><select class="fld" id="profile-memory">${profileMemoryOptions}</select></div>
              <div class="profile-setting profile-java-setting"><label>Java runtime</label><small id="profile-java-hint">Automatic Java is selected to match this profile.</small><div class="profile-java-path" id="profile-java-value">Automatic detection</div><div class="button-pair"><button class="btn" id="profile-java-choose">Choose Java…</button><button class="btn subtle" id="profile-java-reset">Automatic</button></div></div>
              <div class="profile-setting"><label for="profile-graphics">Graphics preset</label><small>Low-impact settings are written before this profile starts and restored when you switch back to default.</small><select class="fld" id="profile-graphics"><option value="default">Keep current game settings</option><option value="intel-uhd-low">Intel UHD · low impact</option></select></div>
            </div>
            <div class="profile-preset-note" id="profile-preset-note" role="status">No profile selected yet.</div>
          </div>
          <div class="settings-callout"><span class="note-mark">i</span><span>Java overrides must match the profile's required major version. The Intel UHD preset targets integrated graphics with 6-chunk view distance, 4-chunk simulation, low effects, and a 60 FPS cap.</span></div>
          <div class="crash-helper-card"><div><h4>Last launch diagnostics</h4><p>Analyze the selected profile's local Bee Launcher log. The explanation is rule-based and shows a redacted excerpt.</p></div><button class="btn" id="diagnose-last-launch">Explain last launch</button><div class="crash-diagnosis" id="crash-diagnosis" role="status" hidden></div></div>
        </section>
        <section class="settings-page" id="settings-appearance" data-settings-page="appearance" hidden>
          <div class="settings-page-heading"><div><small>LOOK & FEEL</small><h3>Appearance</h3><p>Preview your changes as you make them; they save automatically.</p></div><span class="settings-chip settings-chip-live"><i></i> AUTO-SAVING</span></div>
          <div class="setting-card appearance-card"><div class="card-heading"><h4>Launcher wallpaper</h4><p>Choose a built-in scene or use an image from your PC.</p></div><div class="thumbs" id="wallpaper-grid">${options}</div><div class="wallpaper-actions"><span class="path-value" id="wallpaper-name">Built-in design wallpaper</span><div class="button-pair"><button class="btn" id="wallpaper-select">Choose image</button><button class="btn subtle" id="wallpaper-reset">Restore default</button></div></div></div>
          <div class="setting-card setting-list appearance-controls"><div class="setting-row"><div><h4>Play button colour</h4><small>Choose the launcher accent colour.</small></div><div class="swatches" id="accent-swatches">${accents}</div></div><div class="setting-row"><div><label for="glass-opacity">Panel opacity</label><small>Increase it for a clearer, more readable panel.</small></div><div class="range-control"><output id="glass-opacity-value">${appearance.opacity}%</output><input type="range" id="glass-opacity" min="20" max="90" value="${appearance.opacity}" aria-label="Panel opacity"></div></div><div class="setting-row"><div><label for="wallpaper-brightness">Wallpaper brightness</label><small>Adjust the background behind the controls.</small></div><div class="range-control"><output id="wallpaper-brightness-value">${appearance.brightness}%</output><input type="range" id="wallpaper-brightness" min="60" max="120" value="${appearance.brightness}" aria-label="Wallpaper brightness"></div></div></div>
        </section>
        <section class="settings-page" id="settings-folder-page" data-settings-page="folder" hidden>
          <div class="settings-page-heading"><div><small>LOCAL FILES</small><h3>Minecraft folder</h3><p>Choose the existing game folder Bee Launcher should scan.</p></div></div>
          <div class="setting-card folder-card"><div class="folder-icon" aria-hidden="true"><span></span><span></span><span></span></div><div class="folder-details"><b>Current game folder</b><p class="path-value" id="settings-path">${escapeHtml(friendlyPath(appStatus.gameDirectory))}</p><small>Auto-detection checks your populated Roaming and LocalAppData folders.</small></div><div class="folder-actions"><button class="btn" id="settings-folder">Choose folder</button><button class="btn subtle" id="settings-rescan">Rescan profiles</button></div></div>
          <div class="folder-status"><div><span class="status-dot" id="java-dot"></span><span id="java-status">Checking Java…</span></div><div><b id="profile-count">${appStatus.installedVersions.length}</b><span>profiles detected</span></div></div>
          <div class="settings-callout"><span class="note-mark">i</span><span>Changing this location does not move or delete your existing worlds, mods, saves, or settings.</span></div>
        </section>
        <section class="settings-page" id="settings-about" data-settings-page="about" hidden>
          <div class="settings-page-heading"><div><small>INFORMATION</small><h3>About Bee Launcher</h3><p>A local launcher for Java profiles already installed on this PC.</p></div></div>
          <div class="about-grid"><article class="setting-card"><span class="about-badge available">SUPPORTED</span><h4>Local launcher features</h4><ul><li>Installed vanilla and Fabric profiles</li><li>Local player name and custom skin library</li><li>Wallpaper and game launch preferences</li><li>Profile file checks and repair where supported</li><li>Separate modpack profiles</li></ul></article><article class="setting-card"><span class="about-badge limited">NOT CONNECTED</span><h4>Online account features</h4><ul><li>Microsoft or TLauncher sign-in</li><li>Servers requiring account authentication</li><li>Cloud skins, Realms, or online services</li></ul></article></div>
          <div class="settings-callout"><span class="note-mark">i</span><span>Offline mode does not replace missing game files or Java. Each installed profile must still be complete and compatible.</span></div>
        </section>
      </div>
    </div>
    <div class="settings-footer"><span>Profile and game options save with this button. Appearance changes save instantly.</span><div class="button-pair"><button class="btn subtle" id="close-settings">Close</button><button class="btn pri" id="save-settings">Save changes</button></div></div>`;
}

function bindSettings() {
  $('#settings-folder').addEventListener('click', pickGameFolder);
  $('#settings-rescan').addEventListener('click', () => refreshStatus(true));
  $('#avatar-upload').addEventListener('click', pickAvatar);
  $('#avatar-reset').addEventListener('click', resetAvatar);
  $('#wallpaper-select').addEventListener('click', pickWallpaper);
  $('#wallpaper-reset').addEventListener('click', resetWallpaper);
  $('#save-settings').addEventListener('click', saveSettings);
  $('#close-settings').addEventListener('click', () => openPanel('play'));
  document.querySelectorAll('[data-settings-tab]').forEach((button) => button.addEventListener('click', () => {
    document.querySelectorAll('[data-settings-tab]').forEach((item) => {
      if (item === button) item.setAttribute('aria-current', 'page');
      else item.removeAttribute('aria-current');
    });
    document.querySelectorAll('[data-settings-page]').forEach((page) => {
      page.hidden = page.dataset.settingsPage !== button.dataset.settingsTab;
      page.classList.toggle('active', !page.hidden);
    });
  }));
  $('#memory-range').addEventListener('input', (event) => { $('#memory-value').textContent = `${event.target.value} GB`; });
  $('#profile-launch-select').addEventListener('change', (event) => { profileLaunchSelectedKey = event.target.value; renderProfileLaunchControls(); });
  $('#profile-memory').addEventListener('change', (event) => updateProfileLaunchDraft('maxMemoryGb', event.target.value === '' ? null : Number(event.target.value)));
  $('#profile-graphics').addEventListener('change', (event) => updateProfileLaunchDraft('graphicsPreset', event.target.value));
  $('#profile-java-choose').addEventListener('click', pickProfileJava);
  $('#profile-java-reset').addEventListener('click', () => updateProfileLaunchDraft('javaPath', null));
  $('#diagnose-last-launch').addEventListener('click', explainLastLaunch);
  $('#glass-opacity').addEventListener('input', (event) => {
    appearance.opacity = Number(event.target.value);
    $('#glass-opacity-value').textContent = `${appearance.opacity}%`;
    applyAppearance();
    saveAppearance();
  });
  $('#wallpaper-brightness').addEventListener('input', (event) => {
    appearance.brightness = Number(event.target.value);
    $('#wallpaper-brightness-value').textContent = `${appearance.brightness}%`;
    applyAppearance();
    saveAppearance();
  });
  document.querySelectorAll('[data-accent]').forEach((button) => button.addEventListener('click', () => {
    appearance.accent = button.dataset.accent;
    saveAppearance();
    applyAppearance();
    document.querySelectorAll('[data-accent]').forEach((item) => item.setAttribute('aria-pressed', String(item === button)));
  }));
  document.querySelectorAll('[data-wallpaper]').forEach((button) => button.addEventListener('click', () => selectStockWallpaper(button.dataset.wallpaper)));
}

async function saveSettings() {
  const name = $('#settings-username').value.trim();
  if (!validatePlayerName(name)) {
    notify('Player name must be 3–16 letters, numbers, or underscores.');
    return;
  }
  const width = Number($('#resolution-width').value);
  const height = Number($('#resolution-height').value);
  if (!Number.isInteger(width) || width < 800 || width > 7680 || !Number.isInteger(height) || height < 600 || height > 4320) {
    notify('Resolution must be between 800×600 and 7680×4320.');
    return;
  }
  try {
    let saved = await invoke('save_launcher_settings', {
      maxMemoryGb: Number($('#memory-range').value),
      width,
      height,
      fullscreen: $('#fullscreen').checked,
    });
    const availableProfileKeys = new Set((appStatus.installedVersions || []).map(profileKeyOf));
    for (const key of availableProfileKeys) {
      const next = profileLaunchDrafts[key] || profileLaunchSettings[key] || { javaPath: null, maxMemoryGb: null, graphicsPreset: 'default' };
      const previous = profileLaunchSettings[key] || { javaPath: null, maxMemoryGb: null, graphicsPreset: 'default' };
      if (next.javaPath === previous.javaPath && next.maxMemoryGb === previous.maxMemoryGb && (next.graphicsPreset || 'default') === (previous.graphicsPreset || 'default')) continue;
      saved = await invoke('save_profile_launch_settings', { profileKey: key, profileSettings: next });
      profileLaunchSettings = structuredClone(saved.profileSettings || {});
    }
    reflectSettings(saved);
    updateProfileName(name);
    openPanel('play');
    notify('Launcher settings saved.');
  } catch (error) {
    notify(`Could not save settings: ${error}`, 6500);
  }
}

async function launchVersion(profileKey, source = 'Play') {
  if (busy || !profileKey) return;
  const profile = appStatus.installedVersions.find((item) => profileKeyOf(item) === profileKey);
  if (!profile?.launchable) {
    notify('This profile is detected, but required local files are incomplete. Check Installations.');
    return;
  }
  const versionId = profile.id;
  const profileLabel = profile.instanceName ? `${profile.instanceName} (${versionId})` : versionId;
  const name = playerName();
  if (!validatePlayerName(name)) {
    notify('Set a player name of 3–16 letters, numbers, or underscores in Settings.');
    showSettings();
    return;
  }
  if (!profileJavaReady(profile)) {
    const required = profile.requiredJavaMajor ? `Java ${profile.requiredJavaMajor}` : 'a compatible Java runtime';
    notify(`${required} was not found for this profile. Install it and refresh.`);
    return;
  }

  busy = true;
  const playButton = $('#play-button');
  const progress = $('#pbar');
  playButton.disabled = true;
  $('#quick-play').disabled = true;
  playButton.textContent = 'Preparing…';
  progress.classList.add('on', 'active');
  $('#stat').textContent = `Checking files and starting ${profileLabel}…`;
  try {
    const result = await invoke('launch_version', { profileKey, username: name });
    progress.classList.remove('active');
    $('#pfill').style.width = '100%';
    const logLocation = result.logPath.replaceAll('/', '\\').split('\\').slice(-3).join('\\');
    notify(`${source} started ${profileLabel} as ${name}. Log: ${logLocation}`, 6500);
  } catch (error) {
    progress.classList.remove('active');
    $('#pfill').style.width = '100%';
    notify(`Could not start ${profileLabel}: ${error}`, 15000);
  } finally {
    busy = false;
    window.setTimeout(() => {
      progress.classList.remove('on');
      $('#pfill').style.width = '0';
    }, 900);
    playButton.textContent = 'Play';
    renderVersions();
  }
}

async function pickGameFolder() {
  try {
    const selected = await open({ directory: true, multiple: false, title: 'Choose your .minecraft folder' });
    if (!selected) return;
    appStatus = await invoke('set_game_directory', { path: selected });
    renderStatus();
    if (activePanel === 'settings') updateSettingsPath();
    notify('Minecraft folder updated.');
  } catch (error) {
    notify(`Could not use that folder: ${error}`, 7000);
  }
}

async function pickModpack() {
  try {
    const selected = await open({
      directory: false,
      multiple: false,
      title: 'Choose a Modrinth .mrpack or CurseForge ZIP',
      filters: [{ name: 'Minecraft modpack', extensions: ['mrpack', 'zip'] }],
    });
    if (selected) await installModpack(selected);
  } catch (error) {
    notify(`Could not open that modpack: ${error}`, 8000);
  }
}

async function installModpack(path) {
  if (busy) return;
  busy = true;
  renderVersions();
  const importButton = $('#import-modpack-button');
  if (importButton) importButton.disabled = true;
  const progress = $('#modpack-progress');
  const progressTitle = $('#modpack-progress-title');
  const progressMessage = $('#modpack-progress-message');
  const progressBar = $('#modpack-progress-bar');
  const progressDetail = $('#modpack-progress-detail');
  if (progress) progress.hidden = false;
  if (progressTitle) progressTitle.textContent = 'Preparing separate profile…';
  if (progressMessage) progressMessage.textContent = 'Validating the pack and checking the pinned game version.';
  if (progressBar) progressBar.value = 0;
  if (progressDetail) progressDetail.textContent = '0%';
  let unlisten = () => {};
  try {
    unlisten = await listen('modpack-import-progress', ({ payload }) => {
      if (progressTitle) progressTitle.textContent = payload.stage === 'complete' ? 'Import complete' : `Installing · ${payload.stage}`;
      if (progressMessage) progressMessage.textContent = payload.message;
      if (progressBar) progressBar.value = payload.percent;
      if (progressDetail) progressDetail.textContent = payload.total > 0 ? `${payload.percent}% · ${payload.current}/${payload.total}` : `${payload.percent}%`;
    });
    const result = await invoke('import_modpack', { path });
    selectedVersion = result.profileKey;
    appStatus = await invoke('app_status');
    renderStatus();
    const importWarnings = Array.isArray(result.warnings) ? result.warnings : [];
    if (progressTitle) progressTitle.textContent = 'Modpack installed';
    if (progressMessage) progressMessage.textContent = `${result.instanceName} · Minecraft ${result.minecraftVersion}${result.loaderName ? ` · ${result.loaderName} ${result.loaderVersion}` : ' · Vanilla'}${importWarnings.length ? ` · Compatibility note: ${importWarnings.join(' ')}` : ''}`;
    if (progressBar) progressBar.value = 100;
    if (progressDetail) progressDetail.textContent = `${result.filesInstalled} pack files installed`;
    notify(importWarnings.length ? `${result.instanceName} imported. ${importWarnings.join(' ')}` : `${result.instanceName} imported as a separate profile.`, importWarnings.length ? 15000 : 7500);
  } catch (error) {
    if (progressTitle) progressTitle.textContent = 'Import could not finish';
    if (progressMessage) progressMessage.textContent = String(error);
    if (progressDetail) progressDetail.textContent = 'The incomplete instance was removed; your existing Minecraft folder was not changed.';
    notify(`Could not import modpack: ${error}`, 15000);
  } finally {
    unlisten();
    busy = false;
    if (importButton) importButton.disabled = false;
    renderVersions();
  }
}

async function pickWallpaper() {
  try {
    const selected = await open({
      directory: false,
      multiple: false,
      title: 'Choose a launcher wallpaper',
      filters: [{ name: 'Wallpaper image', extensions: ['png', 'jpg', 'jpeg', 'webp'] }],
    });
    if (!selected) return;
    const dataUrl = await invoke('set_wallpaper', { path: selected });
    settings.wallpaperPath = selected;
    settings.wallpaperName = selected.split(/[\\/]/).pop();
    appearance.wallpaper = 'custom';
    saveAppearance();
    setCustomWallpaper(dataUrl);
    notify('Launcher wallpaper updated.');
  } catch (error) {
    notify(`Could not set wallpaper: ${error}`, 7000);
  }
}

async function normalizedAvatar(dataUrl) {
  const image = new Image();
  image.src = dataUrl;
  await image.decode();
  const cropSize = Math.min(image.naturalWidth, image.naturalHeight);
  if (!cropSize) throw new Error('That image could not be decoded.');
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 256;
  const context = canvas.getContext('2d');
  if (!context) throw new Error('Could not prepare the profile picture.');
  context.drawImage(image, (image.naturalWidth - cropSize) / 2, (image.naturalHeight - cropSize) / 2, cropSize, cropSize, 0, 0, 256, 256);
  return canvas.toDataURL('image/png');
}

async function pickAvatar() {
  try {
    const selected = await open({
      directory: false,
      multiple: false,
      title: 'Choose a local profile picture',
      filters: [{ name: 'Profile picture', extensions: ['png', 'jpg', 'jpeg', 'webp'] }],
    });
    if (!selected) return;
    const source = await invoke('read_local_image', { path: selected });
    const avatar = await normalizedAvatar(source);
    try {
      localStorage.setItem(avatarStorageKey, avatar);
    } catch {
      throw new Error('The picture could not be saved locally. Choose a smaller image or clear launcher storage.');
    }
    renderAvatar();
    notify('Profile picture updated on this PC.');
  } catch (error) {
    notify(`Could not set profile picture: ${error}`, 7000);
  }
}

async function skinFacePreview(dataUrl) {
  const image = new Image();
  image.src = dataUrl;
  await image.decode();
  if (image.naturalWidth !== 64 || ![32, 64].includes(image.naturalHeight)) {
    throw new Error('Skin PNGs must be 64×64, or the older 64×32 format.');
  }
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 64;
  const context = canvas.getContext('2d');
  if (!context) throw new Error('Could not create a skin preview.');
  context.imageSmoothingEnabled = false;
  context.drawImage(image, 8, 8, 8, 8, 0, 0, 64, 64);
  context.drawImage(image, 40, 8, 8, 8, 0, 0, 64, 64);
  return canvas.toDataURL('image/png');
}

async function loadCustomSkins() {
  try {
    const library = await invoke('list_custom_skins');
    customSkins = await Promise.all((library.skins || []).map(async (skin) => {
      try {
        return { ...skin, faceDataUrl: await skinFacePreview(skin.dataUrl) };
      } catch (error) {
        console.warn(`Could not preview custom skin ${skin.name}:`, error);
        return skin;
      }
    }));
    activeSkinId = customSkins.some((skin) => skin.id === library.activeSkinId && skin.faceDataUrl && skin.dataUrl) ? library.activeSkinId : null;
    renderAvatar();
    refreshSkinPreview();
    renderCustomSkins();
  } catch (error) {
    console.warn('Could not load custom skins:', error);
  }
}

function refreshSkinPreview() {
  const skin = customSkins.find((item) => item.id === activeSkinId);
  setSkinPreview(skin?.dataUrl || null, skin?.name || 'Default skin');
}

async function initializePlayerSkinPreview() {
  await loadCustomSkins();
  const activeSkin = customSkins.find((skin) => skin.id === activeSkinId && skin.faceDataUrl && skin.dataUrl);
  await initializeSkinPreview($('#skin-canvas'), $('#skin-showcase'), activeSkin ? { dataUrl: activeSkin.dataUrl, name: activeSkin.name } : null);
}

function renderCustomSkins() {
  const grid = $('#custom-skin-grid');
  if (!grid) return;
  const status = $('#skin-support-status');
  const validName = validatePlayerName(playerName());
  const supportReason = validName ? customSkinSupport.reason : 'Set a valid 3–16 character player name in Settings before applying a skin in-game.';
  const canApplyInGame = customSkinSupport.checked && customSkinSupport.supported && validName;
  if (status) {
    status.className = `skin-support${canApplyInGame ? ' ready' : ''}`;
    status.textContent = supportReason;
  }
  if (!customSkins.length) {
    grid.innerHTML = '<div class="skin-empty"><b>No custom skins yet</b><small>Add a 64×64 or legacy 64×32 PNG. Skins stay in Bee Launcher app data.</small></div>';
    return;
  }
  grid.innerHTML = customSkins.map((skin) => {
    const active = skin.id === activeSkinId;
    return `<article class="skin-card"><img class="skin-texture" src="${escapeHtml(skin.dataUrl)}" alt="${escapeHtml(skin.name)} skin texture"><div class="skin-card-copy"><b>${escapeHtml(skin.name)}</b><small>Minecraft skin PNG · local library</small><div class="button-pair"><button class="btn ${active ? 'pri' : ''}" data-use-skin="${escapeHtml(skin.id)}" ${active ? 'disabled' : ''}>${active ? 'Launcher: in use' : 'Use in launcher'}</button><button class="btn skin-game" data-apply-skin="${escapeHtml(skin.id)}" title="${escapeHtml(supportReason)}" ${canApplyInGame ? '' : 'disabled'}>Use in Minecraft</button><button class="btn subtle" data-remove-skin="${escapeHtml(skin.id)}">Remove</button></div></div></article>`;
  }).join('');
  grid.querySelectorAll('[data-use-skin]').forEach((button) => button.addEventListener('click', () => activateCustomSkin(button.dataset.useSkin)));
  grid.querySelectorAll('[data-apply-skin]').forEach((button) => button.addEventListener('click', () => applyCustomSkinToMinecraft(button.dataset.applySkin)));
  grid.querySelectorAll('[data-remove-skin]').forEach((button) => button.addEventListener('click', () => removeCustomSkin(button.dataset.removeSkin)));
}

async function addCustomSkin() {
  try {
    const selected = await open({
      directory: false,
      multiple: false,
      title: 'Add a Minecraft skin PNG',
      filters: [{ name: 'Minecraft skin', extensions: ['png'] }],
    });
    if (!selected) return;
    const skin = await invoke('import_custom_skin', { path: selected });
    skin.faceDataUrl = await skinFacePreview(skin.dataUrl);
    customSkins = [...customSkins, skin].sort((left, right) => left.name.localeCompare(right.name));
    activeSkinId = skin.id;
    renderAvatar();
    refreshSkinPreview();
    renderCustomSkins();
    notify(`${skin.name} added and used as your Bee Launcher profile picture.`);
  } catch (error) {
    notify(`Could not add that skin: ${error}`, 8000);
  }
}

async function activateCustomSkin(skinId) {
  try {
    await invoke('set_active_custom_skin', { skinId });
    activeSkinId = skinId;
    renderAvatar();
    refreshSkinPreview();
    renderCustomSkins();
    const skin = customSkins.find((item) => item.id === skinId);
    notify(`${skin?.name || 'Skin'} is now your Bee Launcher profile picture.`);
  } catch (error) {
    notify(`Could not use that skin: ${error}`, 7000);
  }
}

async function removeCustomSkin(skinId) {
  try {
    await invoke('remove_custom_skin', { skinId });
    customSkins = customSkins.filter((skin) => skin.id !== skinId);
    if (activeSkinId === skinId) activeSkinId = null;
    renderAvatar();
    refreshSkinPreview();
    renderCustomSkins();
    notify('Custom skin removed from Bee Launcher.');
  } catch (error) {
    notify(`Could not remove that skin: ${error}`, 7000);
  }
}

async function refreshCustomSkinSupport() {
  if (!selectedVersion) {
    customSkinSupport = { supported: false, reason: 'Select a Fabric 26.3 profile from Play to check CustomSkinLoader.', loaderVersion: null, checked: false };
    renderCustomSkins();
    return;
  }
  customSkinSupport = { supported: false, reason: 'Checking the selected profile for CustomSkinLoader…', loaderVersion: null, checked: false };
  renderCustomSkins();
  try {
    customSkinSupport = { ...(await invoke('get_custom_skin_support', { profileKey: selectedVersion })), checked: true };
  } catch (error) {
    customSkinSupport = { supported: false, reason: `Could not check this profile: ${error}`, loaderVersion: null, checked: true };
  }
  renderCustomSkins();
}

async function applyCustomSkinToMinecraft(skinId) {
  const name = playerName();
  if (!validatePlayerName(name)) {
    notify('Set a valid 3–16 character player name in Settings first.');
    showSettings();
    return;
  }
  if (!selectedVersion) {
    notify('Select a Fabric 26.3 profile from Play first.');
    return;
  }
  try {
    const result = await invoke('apply_custom_skin_to_profile', {
      profileKey: selectedVersion,
      username: name,
      skinId,
    });
    lastAppliedSkin = { profileKey: selectedVersion, username: result.username, skinId };
    const backupNote = result.replacedExisting ? ' The previous local skin was backed up in Bee Launcher app data.' : '';
    const configBackupNote = result.configBackupCreated ? ' The previous CustomSkinLoader config was backed up in Bee Launcher app data.' : '';
    const card = $('#skin-application-card');
    if (card) {
      card.hidden = false;
      card.innerHTML = `<div class="skin-application-heading"><div><span class="news-card-source">STAGED · NOT YET CONFIRMED IN-GAME</span><h3>Skin file and load order checked</h3><p>Bee verified that this profile's LocalSkin PNG matches your selected library image and that CustomSkinLoader can prioritize it. This does not prove Minecraft has loaded or rendered it yet.</p></div><button class="btn" id="verify-custom-skin">Check loader log</button></div><div class="skin-stage-details"><div><b>Player</b><span>${escapeHtml(result.username)}</span></div><div><b>Profile</b><span>${escapeHtml(result.profileId)}</span></div><div><b>Skin file</b><code>${escapeHtml(result.skinPath)}</code></div><div><b>Loader config</b><code>${escapeHtml(result.loaderConfigPath)}</code></div></div><div class="skin-application-result" id="skin-application-result" role="status">Fully quit Minecraft, launch this profile, and open a world. Then check the loader's own log here. ${result.restartRequired ? 'The LocalSkin priority was newly prepared, so a full restart is required.' : 'A full restart is the safest way to refresh the current profile cache.'}${backupNote}${configBackupNote}</div>`;
      $('#verify-custom-skin').addEventListener('click', verifyCustomSkinApplication);
    }
    notify(`Skin staged for ${result.username}. Bee verified the local PNG and loader order, but has not confirmed Minecraft loaded it.`, 10000);
  } catch (error) {
    notify(`Could not stage this in-game skin: ${error}`, 10000);
  }
}

async function verifyCustomSkinApplication() {
  if (!lastAppliedSkin) return;
  const output = $('#skin-application-result');
  const button = $('#verify-custom-skin');
  if (!output || !button) return;
  output.textContent = 'Checking the selected profile’s CustomSkinLoader log…';
  button.disabled = true;
  try {
    const result = await invoke('verify_custom_skin_application', lastAppliedSkin);
    let message;
    if (result.loadConfirmed) {
      message = 'Confirmed by CustomSkinLoader’s fresh log: the profile selected the exact local skin file for this player. This confirms the loader reference, not the final rendered appearance in Minecraft.';
    } else if (!result.fileMatchesLibrary || !result.loadOrderReady) {
      message = 'The local file or source priority no longer matches the staged setup. Apply the skin again before testing in-game.';
    } else if (!result.loaderLogFound) {
      message = 'No CustomSkinLoader log exists yet. Fully quit Minecraft, launch this profile, open a world, then check again.';
    } else if (!result.loaderLogUpdatedAfterSkin) {
      message = 'The loader log predates the current skin file, so it cannot confirm this copy. Launch this profile and open a world, then check again.';
    } else if (!result.matchingSkinFingerprint) {
      message = 'A newer loader log exists, but it does not contain the exact fingerprint for this skin file. Check that the player name in Settings matches the name Minecraft is using, then fully restart the profile.';
    } else {
      message = 'The load could not be confirmed from the available evidence.';
    }
    const checks = [
      `${result.fileMatchesLibrary ? '✓' : '×'} PNG still matches the selected library skin`,
      `${result.loadOrderReady ? '✓' : '×'} LocalSkin priority is ready`,
      `${result.loaderLogFound ? '✓' : '×'} CustomSkinLoader log found`,
      `${result.loaderLogUpdatedAfterSkin ? '✓' : '×'} Log is newer than the staged file`,
      `${result.matchingSkinFingerprint ? '✓' : '×'} Exact file fingerprint appears in the log`,
    ];
    output.innerHTML = `<p class="${result.loadConfirmed ? 'verified' : 'unverified'}">${escapeHtml(message)}</p><ul>${checks.map((check) => `<li>${escapeHtml(check)}</li>`).join('')}</ul><small>Loader log: ${escapeHtml(result.loaderLogPath)}</small>`;
  } catch (error) {
    output.textContent = `Could not verify the CustomSkinLoader log: ${error}`;
  } finally {
    button.disabled = false;
  }
}

function resetAvatar() {
  localStorage.removeItem(avatarStorageKey);
  renderAvatar();
  notify('Default profile picture restored.');
}

async function selectStockWallpaper(id) {
  try {
    if (settings.wallpaperPath) await invoke('reset_wallpaper');
    settings.wallpaperPath = null;
    settings.wallpaperName = null;
    applyStockWallpaper(id);
    reflectSettings(settings);
  } catch (error) {
    notify(`Could not change wallpaper: ${error}`, 7000);
  }
}

async function resetWallpaper() {
  try {
    await invoke('reset_wallpaper');
    settings.wallpaperPath = null;
    settings.wallpaperName = null;
    appearance.wallpaper = 'sunset';
    saveAppearance();
    applyStockWallpaper('sunset');
    reflectSettings(settings);
    notify('Built-in design wallpaper restored.');
  } catch (error) {
    notify(`Could not reset wallpaper: ${error}`);
  }
}

async function loadPreferences() {
  try {
    const prefs = await invoke('get_launcher_settings');
    reflectSettings(prefs);
    if (prefs.wallpaperPath) {
      const wallpaper = await invoke('get_wallpaper_data');
      if (wallpaper) setCustomWallpaper(wallpaper);
    } else {
      applyStockWallpaper(appearance.wallpaper);
    }
  } catch (error) {
    console.warn('Could not load launcher settings:', error);
    applyStockWallpaper(appearance.wallpaper);
  }
}

function showSettings() {
  openPanel('settings');
  $('#settings-username').value = playerName();
  updateSettingsPath();
}


versionSelect.addEventListener('change', () => {
  selectedVersion = versionSelect.value;
  renderVersions();
  if (activePanel === 'skins') refreshCustomSkinSupport();
});
$('#play-button').addEventListener('click', () => launchVersion(selectedVersion, 'Play'));
$('#quick-play').addEventListener('click', () => launchVersion(selectedVersion, 'Quick Play'));
$('#settings-shortcut').addEventListener('click', showSettings);
bindWindowControls();
restoreWindowSize();
applyDefaultWindowIcon();
document.querySelectorAll('#sub button[data-s]').forEach((button) => button.addEventListener('click', () => {
  const tab = button.dataset.s;
  if (tab === 'play') openPanel('play');
  else if (tab === 'inst') openPanel('inst');
  else if (tab === 'notes') openPanel('notes');
  else if (tab === 'news') openPanel('news');
  else if (tab === 'realms' || tab === 'skins' || tab === 'mods' || tab === 'servers') {
    openPanel(tab, tab[0].toUpperCase() + tab.slice(1));
  }
}));
window.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && activePanel !== 'play') openPanel('play');
});
sheet.addEventListener('click', (event) => {
  if (event.target.closest('[data-close-panel]')) openPanel('play');
});

updateProfileName(playerName());
applyAppearance();
initializeReferenceArtwork();
applyStockWallpaper(appearance.wallpaper);
refreshStatus();
loadPreferences();
initializePlayerSkinPreview();
