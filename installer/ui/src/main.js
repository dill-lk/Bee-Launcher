import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import './style.css';
import beeLogo from '../../../src/assets/bee-logo.png';
import sunsetBridge from '../../../src/assets/ui-sunset-bridge.jpg';

const root = document.querySelector('#app');
const currentWindow = getCurrentWindow();

root.innerHTML = `
  <div class="window">
    <aside class="brand-panel" style="--hero-image: url('${sunsetBridge}')">
      <div class="brand-glow"></div>
      <div class="brand-top">
        <img class="bee-mark" src="${beeLogo}" alt="Bee Launcher bee logo" />
        <div><span class="eyebrow">BLOCKLINE PRESENTS</span><strong>Bee Launcher</strong></div>
      </div>
      <div class="brand-story">
        <div class="tiny-pill"><span class="status-dot"></span> BUILT FOR YOUR NEXT WORLD</div>
        <h1>Start your<br /><em>own story.</em></h1>
        <p>Your profiles, modpacks, and worlds belong to you. Installing Bee keeps them right where they are.</p>
      </div>
      <div class="brand-footer"><span class="hex-mark">B</span><span>MADE FOR PLAYERS<br /><b>NOT JUST LAUNCHERS</b></span></div>
    </aside>

    <main class="setup-panel">
      <header class="titlebar" data-tauri-drag-region>
        <div class="titlebar-label"><span class="mini-bee">B</span> BEE LAUNCHER SETUP</div>
        <div class="window-actions">
          <button class="window-button" id="minimize" aria-label="Minimize window" title="Minimize">−</button>
          <button class="window-button close-button" id="close" aria-label="Close window" title="Close">×</button>
        </div>
      </header>
      <div class="content-wrap">
        <div class="step-line" id="step-line"><span class="step-index">01</span><span id="step-name">WELCOME</span><span class="step-rule"></span><span class="version-label" id="version-label">VERSION</span></div>
        <div id="page-content" class="page-content">
          <div class="loading-orbit"><span></span><span></span><span></span></div>
          <p class="loading-label">Preparing your setup…</p>
        </div>
        <footer class="legal-row"><span>© 2026 Blockline</span><span>NO ADMIN ACCESS REQUIRED</span><span class="secure-label"><i></i> LOCAL INSTALL</span></footer>
      </div>
    </main>
  </div>
`;

const page = document.querySelector('#page-content');
const stepName = document.querySelector('#step-name');
const versionLabel = document.querySelector('#version-label');

function setStatus(message, isError = false) {
  const status = document.querySelector('#status-message');
  if (!status) return;
  status.textContent = message;
  status.classList.toggle('error', isError);
  status.classList.add('visible');
}

function setBusy(button, busy, label) {
  button.disabled = busy;
  button.classList.toggle('is-busy', busy);
  if (label) button.querySelector('.button-label').textContent = label;
}

function closeWindow() {
  currentWindow.close().catch(() => window.close());
}

document.querySelector('#close').addEventListener('click', closeWindow);
document.querySelector('#minimize').addEventListener('click', () => currentWindow.minimize());
document.querySelectorAll('[data-tauri-drag-region]').forEach((node) => {
  node.addEventListener('mousedown', (event) => {
    if (event.button === 0 && !event.target.closest('button')) currentWindow.startDragging();
  });
});

async function start() {
  try {
    const state = await invoke('installer_state');
    versionLabel.textContent = `V${state.version}`;
    render(state);
  } catch (error) {
    page.innerHTML = `
      <div class="setup-heading"><span class="kicker">BEE LAUNCHER · SETUP</span><h2>Couldn’t load setup.</h2><p class="intro">${escapeHtml(String(error))}</p></div>
      <button class="primary-button" id="exit-button"><span class="button-label">Close setup</span><span class="button-arrow">↗</span></button>
    `;
    document.querySelector('#exit-button').addEventListener('click', closeWindow);
  }
}

function render(state) {
  if (state.uninstall_mode) {
    renderUninstall(state);
  } else {
    renderInstall(state);
  }
}

function renderInstall(state) {
  stepName.textContent = state.already_installed ? 'UPDATE' : 'INSTALL';
  const title = state.already_installed ? 'A fresh hive is ready.' : 'Your next adventure starts here.';
  const intro = state.already_installed
    ? `Install Bee Launcher ${escapeHtml(state.version)} over the current app. Your launcher data and Minecraft worlds stay in place.`
    : 'Install the launcher for this Windows account. No administrator access is needed, and your existing Minecraft folder will stay untouched.';
  page.innerHTML = `
    <div class="setup-heading">
      <span class="kicker">${state.already_installed ? 'WELCOME BACK' : 'A BETTER WAY TO PLAY'}</span>
      <h2>${title}</h2>
      <p class="intro">${intro}</p>
    </div>
    <div class="location-card">
      <div class="location-icon"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3.75 6.5A1.75 1.75 0 0 1 5.5 4.75h4.1l1.7 1.8h7.2A1.75 1.75 0 0 1 20.25 8.3v9.2a1.75 1.75 0 0 1-1.75 1.75H5.5a1.75 1.75 0 0 1-1.75-1.75z"/><path d="M4 9h16"/></svg></div>
      <div class="location-copy"><span>INSTALL LOCATION</span><strong id="install-location">${escapeHtml(state.install_dir)}</strong></div>
      <span class="location-badge">THIS ACCOUNT</span>
    </div>
    <div class="options-row">
      <label class="check-option"><input id="desktop-shortcut" type="checkbox" checked /><span class="custom-check"><svg viewBox="0 0 16 16"><path d="m3 8 3.2 3.2L13 4.5"/></svg></span><span><b>Create a desktop shortcut</b><small>You can always launch it from the Start menu.</small></span></label>
    </div>
    <div class="preserve-note"><span class="shield-icon"><svg viewBox="0 0 24 24"><path d="M12 3.5 19 6v5.1c0 4.1-2.9 7.6-7 9.4-4.1-1.8-7-5.3-7-9.4V6z"/><path d="m9 12 2 2 4-4"/></svg></span><span><b>Your game data is not moved or removed.</b><small>Profiles, worlds, saves, and the selected .minecraft folder stay safe.</small></span></div>
    ${!state.release_payload_ready ? '<div class="status-message error visible" id="status-message">This development build does not contain the Windows launcher payload.</div>' : '<div class="status-message" id="status-message" role="status"></div>'}
    <button class="primary-button" id="install-button" ${state.release_payload_ready ? '' : 'disabled'}><span class="button-label">${state.already_installed ? 'Update Bee Launcher' : 'Install Bee Launcher'}</span><span class="button-arrow">↗</span><span class="button-spinner"></span></button>
    <p class="under-button">${state.already_installed ? 'The current app stays available until the new files are ready.' : 'Quick install · No bundled offers · No extra software'}</p>
  `;
  const button = document.querySelector('#install-button');
  button.addEventListener('click', async () => {
    setStatus('Preparing files and creating your Bee shortcuts…');
    setBusy(button, true, state.already_installed ? 'Updating Bee Launcher' : 'Installing Bee Launcher');
    try {
      const result = await invoke('install_launcher', {
        createDesktopShortcut: document.querySelector('#desktop-shortcut').checked,
      });
      stepName.textContent = 'READY';
      page.innerHTML = `
        <div class="success-badge"><span>✓</span> ${state.already_installed ? 'UPDATED' : 'INSTALLED'} · VERSION ${escapeHtml(state.version)}</div>
        <div class="setup-heading success-heading"><span class="kicker">ALL SET</span><h2>Welcome to the hive.</h2><p class="intro">Bee Launcher is installed. Your Minecraft profiles, worlds, and game folder were left untouched.</p></div>
        <div class="success-location"><span class="success-dot"></span><span>Installed to</span><strong>${escapeHtml(result.launch_path)}</strong></div>
        ${result.shortcut_warning ? `<div class="status-message error visible">Bee Launcher was installed, but Windows could not finish setting shortcut identity: ${escapeHtml(result.shortcut_warning)}</div>` : ''}
        <button class="primary-button" id="launch-button"><span class="button-label">Launch Bee Launcher</span><span class="button-arrow">↗</span></button>
        <button class="text-button" id="finish-button">Close setup</button>
      `;
      document.querySelector('#launch-button').addEventListener('click', async () => {
        try {
          await invoke('launch_installed_launcher');
          closeWindow();
        } catch (error) {
          setStatus(String(error), true);
        }
      });
      document.querySelector('#finish-button').addEventListener('click', closeWindow);
    } catch (error) {
      setBusy(button, false, state.already_installed ? 'Update Bee Launcher' : 'Install Bee Launcher');
      setStatus(String(error), true);
    }
  });
}

function renderUninstall(state) {
  stepName.textContent = 'UNINSTALL';
  page.innerHTML = `
    <div class="setup-heading">
      <span class="kicker danger-kicker">UNINSTALL BEE LAUNCHER</span>
      <h2>Leave the worlds.<br /><em>Remove the app.</em></h2>
      <p class="intro">This removes Bee Launcher’s app files, Start menu/desktop shortcuts, and Windows uninstall entry.</p>
    </div>
    <div class="preserve-note preserve-strong"><span class="shield-icon"><svg viewBox="0 0 24 24"><path d="M12 3.5 19 6v5.1c0 4.1-2.9 7.6-7 9.4-4.1-1.8-7-5.3-7-9.4V6z"/><path d="m9 12 2 2 4-5"/></svg></span><span><b>Your Minecraft data stays on this PC.</b><small>Profiles, OUTBREAK, Smallhold, worlds, saves, backups, and all .minecraft files are kept.</small></span></div>
    <div class="location-card uninstall-location"><div class="location-icon"><svg viewBox="0 0 24 24"><path d="M3.75 6.5A1.75 1.75 0 0 1 5.5 4.75h4.1l1.7 1.8h7.2A1.75 1.75 0 0 1 20.25 8.3v9.2a1.75 1.75 0 0 1-1.75 1.75H5.5a1.75 1.75 0 0 1-1.75-1.75z"/><path d="M4 9h16"/></svg></div><div class="location-copy"><span>APP FOLDER</span><strong>${escapeHtml(state.install_dir)}</strong></div></div>
    <div class="status-message" id="status-message" role="status"></div>
    <button class="danger-button" id="uninstall-button"><span class="button-label">Uninstall Bee Launcher</span><span class="button-arrow">→</span><span class="button-spinner"></span></button>
    <button class="text-button cancel-button" id="cancel-button">Keep Bee Launcher</button>
  `;
  const button = document.querySelector('#uninstall-button');
  const cancelButton = document.querySelector('#cancel-button');
  let confirming = false;
  button.addEventListener('click', async () => {
    if (!confirming) {
      confirming = true;
      button.querySelector('.button-label').textContent = 'Confirm uninstall';
      button.classList.add('confirming');
      setStatus('One last check: only the Bee Launcher app files will be removed. Your profiles, worlds, saves, backups, and Minecraft folders will remain.');
      return;
    }
    setBusy(button, true, 'Removing Bee Launcher');
    cancelButton.disabled = true;
    try {
      await invoke('uninstall_launcher');
      page.innerHTML = '<div class="uninstall-done"><span class="success-mark">✓</span><span class="kicker">UNINSTALL STARTED</span><h2>Your worlds are still yours.</h2><p>Bee Launcher is closing and its app files are being removed. Minecraft profiles and worlds were not touched.</p></div>';
    } catch (error) {
      setBusy(button, false, 'Confirm uninstall');
      cancelButton.disabled = false;
      setStatus(String(error), true);
    }
  });
  cancelButton.addEventListener('click', closeWindow);
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (character) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  })[character]);
}

start();
