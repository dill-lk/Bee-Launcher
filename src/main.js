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
const avatarStorageKey = 'blockline.playerAvatar';
let appearance = loadAppearance();
let appStatus = { gameDirectory: '', installedVersions: [], javaAvailable: false };
let settings = { maxMemoryGb: 4, width: 1280, height: 720, fullscreen: false, wallpaperPath: null, wallpaperName: null };
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
  const avatar = localStorage.getItem(avatarStorageKey);
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
  $('#vn').textContent = selected ? (selected.instanceName ? `${selected.instanceName} · ${selected.id}` : selected.id) : 'No profile selected';
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
    list.innerHTML = '<div class="item"><span class="dot" style="background:#ffcf4a"></span><div><b>No imported packs yet</b><small>Choose a Modrinth .mrpack or Bee Launcher ZIP to install one.</small></div></div>';
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
    notify('Modpack profile selected. Press Play when it is ready.');
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
    if (showMessage) {
      const ready = appStatus.installedVersions.filter((version) => version.launchable).length;
      notify(`${appStatus.installedVersions.length} profile${appStatus.installedVersions.length === 1 ? '' : 's'} detected · ${ready} ready to launch.`);
    }
  } catch (error) {
    notify(`Could not scan your Minecraft folder: ${error}`, 7000);
  }
}

function reflectSettings(prefs) {
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
  sheet.classList.toggle('big', name === 'settings' || name === 'inst' || name === 'mods');
  if (name === 'settings') {
    sheet.classList.add('settings-panel');
    sheet.innerHTML = settingsMarkup();
    renderAvatar();
    bindSettings();
    reflectSettings(settings);
  } else if (name === 'inst') {
    sheet.innerHTML = `${panelHeader('Installations', 'Local Minecraft profiles and imported packs')}<div class="profile-list" id="installations-list"></div><div class="panel-actions"><button class="btn" id="rescan-button">Rescan</button><button class="btn" id="choose-folder-modal">Choose folder</button><button class="btn pri" id="import-modpack-from-inst">Import Modpack</button></div>`;
    renderInstallations();
    $('#rescan-button').addEventListener('click', () => refreshStatus(true));
    $('#choose-folder-modal').addEventListener('click', pickGameFolder);
    $('#import-modpack-from-inst').addEventListener('click', () => openPanel('mods'));
  } else if (name === 'mods') {
    sheet.innerHTML = `${panelHeader('Import a modpack', 'Modrinth .mrpack or Bee Launcher ZIP')}
      <div class="pack-import-card"><div class="pack-import-copy"><b>Install as a separate profile</b><small>The pack gets its own game folder. Your existing .minecraft mods, configs, and saves stay untouched.</small></div><button class="btn pri" id="import-modpack-button">Choose .mrpack or ZIP</button>
        <div class="import-progress" id="modpack-progress" hidden><div class="import-progress-copy"><b id="modpack-progress-title">Getting ready…</b><small id="modpack-progress-message">This may take a while while Minecraft files download.</small></div><progress id="modpack-progress-bar" max="100" value="0"></progress><small id="modpack-progress-detail">0%</small></div>
      </div>
      <div class="section-intro modpack-list-heading"><h3>Imported profiles</h3><p>Select one here or choose it from the Play profile menu.</p></div><div class="profile-list" id="modpack-list"></div>
      <div class="local-note"><span class="note-mark">i</span><span>Fabric and vanilla Modrinth packs are supported; Forge, NeoForge, and Quilt are not supported yet. Imported mod files are checked against their manifest hashes.</span></div>`;
    $('#import-modpack-button').addEventListener('click', pickModpack);
    renderImportedPacks();
  } else {
    const messages = {
      realms: 'Realms is not available in this local launcher. It only starts profiles already installed on this device.',
      skins: 'Skin upload and account sync are not available in offline profile mode. Set the local player name in Settings.',
      servers: 'This local launcher does not browse or join servers. Online play requiring Microsoft or TLauncher authentication is unsupported.',
      notes: 'This local build does not load or invent Minecraft patch notes. Use the Minecraft website or official launcher for current release notes.',
    };
    sheet.innerHTML = `${panelHeader(title || 'Unavailable')}<div class="item"><span class="dot" style="background:#ffcf4a"></span><div><b>Not part of this local build</b><small>${escapeHtml(messages[name] || 'This feature is not available in this local launcher.')}</small></div></div>`;
  }
  const close = sheet.querySelector('[data-close-panel]');
  if (close) close.addEventListener('click', () => openPanel('play'));
}

function settingsMarkup() {
  const options = wallpaperOptions.map((item) => `<button class="wallpaper-card" data-wallpaper="${item.id}" aria-pressed="false"><img src="${escapeHtml(item.src)}" alt=""><span>${escapeHtml(item.name)}</span></button>`).join('');
  const accents = Object.entries(accentColors).map(([key, color]) => `<button class="accent-swatch" data-accent="${key}" aria-label="${key} accent" title="${key[0].toUpperCase()}${key.slice(1)}" aria-pressed="${key === appearance.accent}" style="background:linear-gradient(${color[3]},${color[2]})"></button>`).join('');
  return `${panelHeader('Settings', 'Make Bee Launcher yours')}
    <div class="settings-layout">
      <nav class="settings-rail" aria-label="Settings sections">
        <button data-settings-tab="profile" aria-controls="settings-profile" aria-current="page"><span class="rail-mark">1</span><span>Profile</span></button>
        <button data-settings-tab="game" aria-controls="settings-game"><span class="rail-mark">2</span><span>Game</span></button>
        <button data-settings-tab="appearance" aria-controls="settings-appearance"><span class="rail-mark">3</span><span>Appearance</span></button>
        <button data-settings-tab="folder" aria-controls="settings-folder-page"><span class="rail-mark">4</span><span>Minecraft folder</span></button>
        <button data-settings-tab="about" aria-controls="settings-about"><span class="rail-mark">i</span><span>About</span></button>
      </nav>
      <div class="settings-pages">
        <section class="settings-page active" id="settings-profile" data-settings-page="profile">
          <div class="section-intro"><h3>Your local profile</h3><p>Choose how you appear in this launcher and in offline play.</p></div>
          <div class="setting-card profile-card">
            <div class="field-stack"><label for="settings-username">Player name</label><input class="fld" id="settings-username" maxlength="16" autocomplete="off" value="${escapeHtml(playerName())}" placeholder="Your Minecraft name"><small>Use 3–16 letters, numbers, or underscores.</small></div>
            <div class="avatar-card"><span class="face" id="avatar-preview" aria-label="Profile picture preview"></span><div class="avatar-copy"><b>Profile picture</b><small>Shown here only; it stays on this PC.</small><div class="button-pair"><button class="btn" id="avatar-upload">Choose picture</button><button class="btn subtle" id="avatar-reset">Reset</button></div></div></div>
          </div>
          <div class="local-note"><span class="note-mark">i</span><span>This is a local, offline profile. Bee Launcher does not read or store Microsoft or TLauncher sign-in tokens.</span></div>
        </section>
        <section class="settings-page" id="settings-game" data-settings-page="game" hidden>
          <div class="section-intro"><h3>Game launch</h3><p>These options are used the next time you start a profile.</p></div>
          <div class="setting-card setting-list">
            <div class="setting-row"><div><label for="memory-range">Memory limit</label><small>Maximum memory given to Minecraft.</small></div><div class="range-control"><output id="memory-value">${settings.maxMemoryGb || 4} GB</output><input type="range" id="memory-range" min="1" max="16" step="1" value="${settings.maxMemoryGb || 4}" aria-label="Maximum memory"></div></div>
            <div class="setting-row"><div><label for="resolution-width">Window size</label><small>Width and height in pixels.</small></div><div class="resolution-inputs"><input class="fld" type="number" min="800" max="7680" id="resolution-width" aria-label="Window width" value="${settings.width || 1280}"><span>×</span><input class="fld" type="number" min="600" max="4320" id="resolution-height" aria-label="Window height" value="${settings.height || 720}"></div></div>
            <div class="setting-row"><div><label for="fullscreen">Start in fullscreen</label><small>You can change this later in Minecraft.</small></div><label class="tg"><input type="checkbox" id="fullscreen" ${settings.fullscreen ? 'checked' : ''}><i></i></label></div>
          </div>
          <div class="local-note"><span class="note-mark">i</span><span>Changing memory is not a guarantee of higher performance; the profile and Java runtime still determine whether a game can launch.</span></div>
        </section>
        <section class="settings-page" id="settings-appearance" data-settings-page="appearance" hidden>
          <div class="section-intro"><h3>Make it yours</h3><p>Wallpaper and appearance changes preview immediately and save on this PC.</p></div>
          <div class="setting-card appearance-card"><div class="card-heading"><h4>Wallpaper</h4><p>Pick a scene or use one of your own images.</p></div><div class="thumbs" id="wallpaper-grid">${options}</div><div class="wallpaper-actions"><span class="path-value" id="wallpaper-name">Built-in design wallpaper</span><div class="button-pair"><button class="btn" id="wallpaper-select">Choose image</button><button class="btn subtle" id="wallpaper-reset">Restore</button></div></div></div>
          <div class="setting-card setting-list appearance-controls"><div class="setting-row"><div><h4>Play button colour</h4><small>Choose an accent colour.</small></div><div class="swatches" id="accent-swatches">${accents}</div></div><div class="setting-row"><div><label for="glass-opacity">Panel opacity</label><small>Higher values make glass easier to read.</small></div><div class="range-control"><output id="glass-opacity-value">${appearance.opacity}%</output><input type="range" id="glass-opacity" min="20" max="90" value="${appearance.opacity}" aria-label="Panel opacity"></div></div><div class="setting-row"><div><label for="wallpaper-brightness">Wallpaper brightness</label><small>Adjust the scene behind the controls.</small></div><div class="range-control"><output id="wallpaper-brightness-value">${appearance.brightness}%</output><input type="range" id="wallpaper-brightness" min="60" max="120" value="${appearance.brightness}" aria-label="Wallpaper brightness"></div></div></div>
        </section>
        <section class="settings-page" id="settings-folder-page" data-settings-page="folder" hidden>
          <div class="section-intro"><h3>Minecraft folder</h3><p>Bee Launcher reads the installed profiles already on this PC.</p></div>
          <div class="setting-card folder-card"><div class="folder-icon" aria-hidden="true"><span></span><span></span><span></span></div><div class="folder-details"><b>Current game folder</b><p class="path-value" id="settings-path">${escapeHtml(friendlyPath(appStatus.gameDirectory))}</p><small>Auto-detection checks the populated Roaming and LocalAppData folders.</small></div><div class="folder-actions"><button class="btn" id="settings-folder">Choose folder</button><button class="btn subtle" id="settings-rescan">Rescan</button></div></div>
          <div class="folder-status"><div><span class="status-dot" id="java-dot"></span><span id="java-status">Checking Java…</span></div><div><b id="profile-count">${appStatus.installedVersions.length}</b><span>installed profiles found</span></div></div>
          <div class="local-note"><span class="note-mark">i</span><span>Choosing a different folder does not move, delete, or change your existing worlds, mods, or settings.</span></div>
        </section>
        <section class="settings-page" id="settings-about" data-settings-page="about" hidden>
          <div class="section-intro"><h3>About Bee Launcher</h3><p>A small local launcher for Minecraft Java profiles installed on this device.</p></div>
          <div class="about-grid"><article class="setting-card"><span class="about-badge available">Available</span><h4>What works here</h4><ul><li>Installed vanilla and Fabric profiles</li><li>Local player name and profile picture</li><li>Wallpaper and launch preferences</li><li>Profile-declared file checks and repair</li></ul></article><article class="setting-card"><span class="about-badge limited">Not supported</span><h4>Online account features</h4><ul><li>Microsoft or TLauncher sign-in</li><li>Servers that require authenticated accounts</li><li>Cloud skins, Realms, and online services</li></ul></article></div>
          <div class="local-note"><span class="note-mark">i</span><span>Offline mode does not make a profile compatible by itself. Minecraft and Java files must still be present and valid.</span></div>
        </section>
      </div>
    </div>
    <div class="settings-footer"><span>Appearance saves as you change it. Player and game settings save when you choose Save changes.</span><div class="button-pair"><button class="btn subtle" id="close-settings">Close</button><button class="btn pri" id="save-settings">Save changes</button></div></div>`;
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
    const saved = await invoke('save_launcher_settings', {
      maxMemoryGb: Number($('#memory-range').value),
      width,
      height,
      fullscreen: $('#fullscreen').checked,
    });
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
      title: 'Choose a Modrinth .mrpack or Bee Launcher ZIP',
      filters: [{ name: 'Modrinth modpack or Bee Launcher bundle', extensions: ['mrpack', 'zip'] }],
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
  if (progressTitle) progressTitle.textContent = 'Preparing separate instance…';
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
    if (progressTitle) progressTitle.textContent = 'Modpack installed';
    if (progressMessage) progressMessage.textContent = `${result.instanceName} · Minecraft ${result.minecraftVersion}${result.fabricLoader ? ` · Fabric ${result.fabricLoader}` : ''}`;
    if (progressBar) progressBar.value = 100;
    if (progressDetail) progressDetail.textContent = `${result.filesInstalled} mod files verified`;
    notify(`${result.instanceName} imported as a separate profile.`, 7500);
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

function toggleNews(force) {
  const drawer = $('#drawer');
  const openState = typeof force === 'boolean' ? force : !drawer.classList.contains('open');
  drawer.classList.toggle('open', openState);
  $('#news-button').setAttribute('aria-expanded', String(openState));
  $('#chev').style.transform = openState ? 'rotate(180deg)' : '';
}

versionSelect.addEventListener('change', () => {
  selectedVersion = versionSelect.value;
  renderVersions();
});
$('#play-button').addEventListener('click', () => launchVersion(selectedVersion, 'Play'));
$('#quick-play').addEventListener('click', () => launchVersion(selectedVersion, 'Quick Play'));
$('#settings-shortcut').addEventListener('click', showSettings);
bindWindowControls();
restoreWindowSize();
applyDefaultWindowIcon();
$('#news-button').addEventListener('click', () => toggleNews());
$('#news-close').addEventListener('click', () => toggleNews(false));
document.querySelectorAll('#sub button[data-s]').forEach((button) => button.addEventListener('click', () => {
  const tab = button.dataset.s;
  if (tab === 'play') openPanel('play');
  else if (tab === 'inst') openPanel('inst');
  else if (tab === 'realms' || tab === 'skins' || tab === 'mods' || tab === 'servers' || tab === 'notes') {
    const title = tab === 'notes' ? 'Patch notes' : tab[0].toUpperCase() + tab.slice(1);
    openPanel(tab, title);
  }
}));
window.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') {
    toggleNews(false);
    if (activePanel !== 'play') openPanel('play');
  }
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
