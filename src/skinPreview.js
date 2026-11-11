import { IdleAnimation, SkinViewer } from 'skinview3d';

let viewer = null;
let previewHost = null;
let fallback = null;
let previewName = null;
let resizeObserver = null;
let targetLook = { x: 0, y: 0 };
let loadGeneration = 0;

function makeDefaultSkin() {
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 64;
  const context = canvas.getContext('2d');
  if (!context) return '';

  const fill = (x, y, width, height, color) => {
    context.fillStyle = color;
    context.fillRect(x, y, width, height);
  };

  // A small, fully local classic-style player texture used until a library skin is selected.
  fill(0, 0, 32, 16, '#59391f');
  fill(8, 8, 8, 8, '#d6a477');
  fill(8, 8, 8, 2, '#59391f');
  fill(10, 11, 1, 1, '#2a211b');
  fill(13, 11, 1, 1, '#2a211b');
  fill(11, 14, 2, 1, '#9a5543');
  fill(16, 16, 24, 16, '#397b4a');
  fill(20, 20, 8, 12, '#4f9b5c');
  fill(0, 16, 16, 16, '#397b4a');
  fill(4, 20, 4, 12, '#4f9b5c');
  fill(40, 16, 16, 16, '#397b4a');
  fill(44, 20, 4, 12, '#4f9b5c');
  fill(0, 16, 16, 4, '#4b709f');
  fill(4, 16, 4, 4, '#638fc4');
  fill(16, 48, 16, 16, '#4b709f');
  fill(20, 52, 4, 12, '#638fc4');
  return canvas.toDataURL('image/png');
}

function setFallbackSkin(dataUrl) {
  if (!fallback) return;
  fallback.style.setProperty('--preview-skin', `url("${dataUrl || makeDefaultSkin()}")`);
}

function resizeViewer() {
  if (!viewer || !previewHost) return;
  const bounds = previewHost.getBoundingClientRect();
  const width = Math.max(1, Math.floor(bounds.width));
  const height = Math.max(1, Math.floor(bounds.height));
  if (width !== viewer.width || height !== viewer.height) viewer.setSize(width, height);
}

export async function initializeSkinPreview(canvas, host, initialSkin = null) {
  previewHost = host;
  fallback = host?.querySelector('#skin-preview-fallback') || null;
  previewName = host?.querySelector('#skin-preview-name') || null;
  if (!canvas || !host) return false;
  const initialDataUrl = initialSkin?.dataUrl || makeDefaultSkin();
  const initialName = initialSkin?.name || 'Default skin';
  setFallbackSkin(initialDataUrl);
  if (previewName) previewName.textContent = initialName;
  host.classList.add('loading');
  host.classList.remove('ready');

  try {
    const idle = new IdleAnimation();
    idle.speed = 0.72;
    idle.addAnimation((player) => {
      const head = player.skin.head;
      const ease = 0.08;
      const headTargetX = targetLook.y * 0.24;
      const headTargetY = targetLook.x * 0.48;
      head.rotation.x += (headTargetX - head.rotation.x) * ease;
      head.rotation.y += (headTargetY - head.rotation.y) * ease;
      const bodyTarget = targetLook.x * 0.16;
      player.rotation.y += (bodyTarget - player.rotation.y) * 0.025;
    });

    viewer = new SkinViewer({
      canvas,
      width: Math.max(1, Math.floor(host.clientWidth)),
      height: Math.max(1, Math.floor(host.clientHeight)),
      pixelRatio: Math.min(window.devicePixelRatio || 1, 1.5),
      model: 'auto-detect',
      fov: 42,
      zoom: 0.76,
      enableControls: true,
      animation: idle,
      background: null,
    });
    viewer.autoRotate = false;
    viewer.controls.enableZoom = false;
    viewer.controls.enablePan = false;
    viewer.controls.enableDamping = true;
    viewer.controls.rotateSpeed = 0.62;
    viewer.globalLight.intensity = 2.2;
    viewer.cameraLight.intensity = 0.7;
    viewer.playerObject.rotation.y = 0;
    canvas.setAttribute('aria-label', 'Animated 3D preview of your Minecraft skin. Move the pointer to make the character look around, or drag to rotate.');

    host.addEventListener('pointermove', (event) => {
      const rect = host.getBoundingClientRect();
      if (!rect.width || !rect.height) return;
      targetLook = {
        x: Math.max(-1, Math.min(1, ((event.clientX - rect.left) / rect.width - 0.5) * 2)),
        y: Math.max(-1, Math.min(1, ((event.clientY - rect.top) / rect.height - 0.5) * 2)),
      };
    });
    host.addEventListener('pointerleave', () => { targetLook = { x: 0, y: 0 }; });
    host.addEventListener('pointerdown', (event) => {
      if (event.button === 0) host.classList.add('is-turning');
    });
    window.addEventListener('pointerup', () => host.classList.remove('is-turning'));

    resizeObserver = new ResizeObserver(resizeViewer);
    resizeObserver.observe(host);
    document.addEventListener('visibilitychange', () => {
      if (viewer) viewer.renderPaused = document.visibilityState === 'hidden';
    });
    await viewer.loadSkin(initialDataUrl, { model: 'auto-detect' });
    canvas.hidden = false;
    if (fallback) fallback.hidden = true;
    host.classList.remove('loading');
    host.classList.add('ready');
    return true;
  } catch (error) {
    console.warn('Could not initialize the 3D skin preview:', error);
    viewer = null;
    canvas.hidden = true;
    if (fallback) fallback.hidden = false;
    host.classList.remove('loading');
    host.classList.add('ready');
    return false;
  }
}

export async function setSkinPreview(dataUrl, name = 'Default skin') {
  const skin = dataUrl || makeDefaultSkin();
  setFallbackSkin(skin);
  if (previewName) previewName.textContent = name || 'Default skin';
  if (!viewer) return;
  const generation = ++loadGeneration;
  previewHost?.classList.add('loading');
  try {
    await viewer.loadSkin(skin, { model: 'auto-detect' });
    if (generation !== loadGeneration) return;
    const canvas = previewHost?.querySelector('canvas');
    if (canvas) canvas.hidden = false;
    if (fallback) fallback.hidden = true;
    previewHost?.classList.remove('loading');
    previewHost?.classList.add('ready');
  } catch (error) {
    console.warn('Could not show the selected skin in 3D:', error);
    if (generation !== loadGeneration) return;
    const canvas = previewHost?.querySelector('canvas');
    if (canvas) canvas.hidden = true;
    if (fallback) fallback.hidden = false;
    previewHost?.classList.remove('loading');
    previewHost?.classList.add('ready');
  }
}
