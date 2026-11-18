// Plomcase 3D case preview. Lazy-loaded by app.js the first time "3D case" is opened.
// Only talks to the app through window.plomcase (getState / buildSvg / buildDiscSvg / onChange).
import * as THREE from 'three';
import {OrbitControls} from 'three/addons/controls/OrbitControls.js';
import {RoomEnvironment} from 'three/addons/environments/RoomEnvironment.js';

const DEFAULT_LID_OPEN = 120;
const PX_PER_MM = 10;
const REBUILD_DELAY = 250;

const num = (value, fallback, min, max) => {
  const n = Number(value);
  return Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : fallback;
};

function rasterizeSvg(svg, widthPx, heightPx, draw) {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(new Blob([svg], {type: 'image/svg+xml;charset=utf-8'}));
    const img = new Image();
    img.onload = () => {
      try {
        const canvas = document.createElement('canvas');
        canvas.width = Math.max(1, Math.round(widthPx));
        canvas.height = Math.max(1, Math.round(heightPx));
        draw(canvas.getContext('2d'), img, canvas);
        resolve(canvas);
      } catch (error) { reject(error); } finally { URL.revokeObjectURL(url); }
    };
    img.onerror = () => { URL.revokeObjectURL(url); reject(new Error('SVG could not be rasterized')); };
    img.src = url;
  });
}

// Quad from four corners (bottom-left, bottom-right, top-right, top-left as seen from its front)
// with an explicit UV rectangle [u0, u1] x [v0, v1].
function quad(bl, br, tr, tl, u0 = 0, u1 = 1, v0 = 0, v1 = 1) {
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.Float32BufferAttribute([...bl, ...br, ...tr, ...tl], 3));
  g.setAttribute('uv', new THREE.Float32BufferAttribute([u0, v0, u1, v0, u1, v1, u0, v1], 2));
  g.setIndex([0, 1, 2, 0, 2, 3]);
  g.computeVertexNormals();
  return g;
}

function box(x0, x1, y0, y1, z0, z1) {
  const g = new THREE.BoxGeometry(Math.max(.01, x1 - x0), Math.max(.01, y1 - y0), Math.max(.01, z1 - z0));
  g.translate((x0 + x1) / 2, (y0 + y1) / 2, (z0 + z1) / 2);
  return g;
}

export function createCasePreview(stage) {
  const api = window.plomcase;
  const renderer = new THREE.WebGLRenderer({antialias: true, alpha: true, preserveDrawingBuffer: false});
  renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.NeutralToneMapping;
  renderer.toneMappingExposure = 1;
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = THREE.PCFShadowMap;
  renderer.setClearColor(0x000000, 0);
  const canvas = renderer.domElement;
  canvas.className = 'case3d-canvas';
  canvas.setAttribute('role', 'img');
  canvas.setAttribute('aria-label', '3D preview of the printed game case with the disc inside');
  stage.prepend(canvas);

  const scene = new THREE.Scene();
  const pmrem = new THREE.PMREMGenerator(renderer);
  const roomEnv = new RoomEnvironment();
  const envTexture = pmrem.fromScene(roomEnv, .04).texture;
  roomEnv.dispose?.();
  pmrem.dispose();
  scene.environment = envTexture;
  scene.environmentIntensity = .7;

  scene.add(new THREE.HemisphereLight(0xffffff, 0x50565a, 1.5));
  const key = new THREE.DirectionalLight(0xffffff, 1.7);
  key.castShadow = true;
  key.shadow.mapSize.set(1024, 1024);
  key.shadow.radius = 6;
  key.shadow.bias = -.0005;
  scene.add(key, key.target);
  const rim = new THREE.DirectionalLight(0xd8f08a, .45);
  scene.add(rim);

  const camera = new THREE.PerspectiveCamera(28, 1, 1, 5000);
  const controls = new OrbitControls(camera, canvas);
  controls.enableDamping = true;
  controls.dampingFactor = .08;
  controls.enablePan = false;

  const floor = new THREE.Mesh(new THREE.PlaneGeometry(1, 1), new THREE.ShadowMaterial({opacity: .32}));
  floor.rotation.x = -Math.PI / 2;
  floor.receiveShadow = true;
  scene.add(floor);

  const lidSlider = stage.querySelector('[data-case3d="lid"]');
  const lidOutput = stage.querySelector('[data-case3d="lid-value"]');
  const lidButton = stage.querySelector('[data-case3d="toggle-lid"]');
  const resetButton = stage.querySelector('[data-case3d="reset"]');

  let caseGroup = null;
  let lidPivot = null;
  let lidAngle = Number(lidSlider?.value) || 0;
  let lidTarget = lidAngle;
  let dims = null;
  let visible = false;
  let dirty = true;
  let frame = 0;
  let rebuildTimer = 0;
  let buildToken = 0;
  let wrapTexture = null;
  let autoCamera = true; // camera follows the lid until the user orbits/zooms
  let discTexture = null;

  function readDims() {
    const s = api.getState() || {};
    const H = num(s.faceHeight, 190, 40, 350);
    const W = num(s.faceWidth, 135, 30, 300);
    const D = num(s.spineWidth, 14, 3, 60);
    const discD = num(s.discDiameter, 50, 5, 400);
    const holeD = Math.min(discD * .8, num(s.discHole, 6.25, 0, 400));
    return {H, W, D, discD, holeD};
  }

  async function makeWrapTexture(d) {
    const drawing = api.buildSvg && api.buildSvg({cropMarks: false});
    if (!drawing || !drawing.svg) return null;
    const trimW = d.W * 2 + d.D;
    const bleed = Math.max(0, (Number(drawing.height) - d.H) / 2) || 0;
    const scale = Math.min(PX_PER_MM, renderer.capabilities.maxTextureSize / trimW, 4096 / trimW);
    const c = await rasterizeSvg(drawing.svg, trimW * scale, d.H * scale, (ctx, img, cv) => {
      ctx.fillStyle = '#ffffff';
      ctx.fillRect(0, 0, cv.width, cv.height);
      ctx.drawImage(img, -bleed * scale, -bleed * scale, Number(drawing.width) * scale, Number(drawing.height) * scale);
    });
    const tex = new THREE.CanvasTexture(c);
    tex.colorSpace = THREE.SRGBColorSpace;
    tex.anisotropy = renderer.capabilities.getMaxAnisotropy();
    return tex;
  }

  async function makeDiscTexture() {
    const drawing = typeof api.buildDiscSvg === 'function' ? api.buildDiscSvg({cropMarks: false}) : null;
    if (!drawing || !drawing.svg) return null;
    const size = Math.min(1024, renderer.capabilities.maxTextureSize);
    // Crop the bleed so the texture square is exactly the trimmed disc diameter (RingGeometry UVs).
    const width = Number(drawing.width), height = Number(drawing.height) || width;
    const bleed = Number.isFinite(Number(drawing.bleed)) ? Number(drawing.bleed) : 0;
    const diameter = Number(drawing.diameter) || Math.max(1, width - 2 * bleed);
    const k = size / diameter;
    const c = await rasterizeSvg(drawing.svg, size, size, (ctx, img, cv) => {
      ctx.fillStyle = '#c9ced3'; // silver shows through transparent parts of the art
      ctx.fillRect(0, 0, cv.width, cv.height);
      ctx.drawImage(img, -bleed * k, -bleed * k, width * k, height * k);
    });
    const tex = new THREE.CanvasTexture(c);
    tex.colorSpace = THREE.SRGBColorSpace;
    tex.anisotropy = renderer.capabilities.getMaxAnisotropy();
    return tex;
  }

  function disposeGroup(group) {
    if (!group) return;
    group.traverse((obj) => {
      if (!obj.isMesh) return;
      obj.geometry.dispose();
      (Array.isArray(obj.material) ? obj.material : [obj.material]).forEach((m) => m.dispose());
    });
    scene.remove(group);
  }

  function buildCase(d, wrap, discArt) {
    const {H, W, D} = d;
    const t = Math.min(1.5, D * .12);
    const T = W * 2 + D;
    const uBack = W / T, uSpine = (W + D) / T;
    const group = new THREE.Group();
    const add = (geometry, material, parent = group) => {
      const mesh = new THREE.Mesh(geometry, material);
      mesh.castShadow = true;
      mesh.receiveShadow = true;
      parent.add(mesh);
      return mesh;
    };

    // Printed insert behind clear plastic: glossy clearcoat over a satin print.
    const art = new THREE.MeshPhysicalMaterial({
      color: wrap ? 0xffffff : 0x3a433b, map: wrap, roughness: .55, metalness: 0,
      clearcoat: 1, clearcoatRoughness: .08, envMapIntensity: .45
    });
    // Clear, slightly frosted shell (like a DVD case) and a light grey inner tray.
    const shell = new THREE.MeshPhysicalMaterial({
      color: 0xeef3f5, roughness: .28, metalness: 0, clearcoat: 1, clearcoatRoughness: .06,
      transparent: true, opacity: .42, depthWrite: false
    });
    const tray = new THREE.MeshPhysicalMaterial({color: 0xb4babe, roughness: .45, clearcoat: .7, clearcoatRoughness: .15});
    const traySeat = new THREE.MeshStandardMaterial({color: 0x9aa1a6, roughness: .5});
    const paper = new THREE.MeshStandardMaterial({color: 0xf2f2ee, roughness: .9});

    const x0 = -W / 2, x1 = W / 2, y0 = -H / 2, y1 = H / 2, z0 = -D / 2, z1 = D / 2;
    // Outside of the base: back (seen from behind, spine on its right) and spine (seen from the left).
    add(quad([x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0], 0, uBack), art);
    add(quad([x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0], uBack, uSpine), art);
    // Inside of the base: tray floor and spine wall, plus top, bottom, and opening-side walls.
    add(box(x0 + .01, x1, y0, y1, z0 + .01, z0 + t), tray);
    add(box(x0 + .01, x0 + t, y0, y1, z0 + t, z1 - t), tray);
    add(box(x0 + t, x1, y1 - t, y1, z0 + t, z1 - t), shell);
    add(box(x0 + t, x1, y0, y0 + t, z0 + t, z1 - t), shell);
    add(box(x1 - t, x1, y0 + t, y1 - t, z0 + t, z1 - t), shell);

    // Disc seat ring and hub at the centre of the tray.
    const discR = d.discD / 2, holeR = d.holeD / 2;
    const floorZ = z0 + t;
    const seat = add(new THREE.RingGeometry(discR + .4, discR + Math.max(1.5, discR * .08), 128), traySeat);
    seat.position.z = floorZ + .02;
    seat.castShadow = false;
    if (holeR > .3) { // no hole, no hub (it would poke through the disc)
      const hubR = Math.max(.25, holeR - .15);
      const hubH = Math.min(Math.max(1.5, (D - 2 * t) * .35), 4);
      const hub = add(new THREE.CylinderGeometry(hubR, hubR, hubH, 48), traySeat);
      hub.rotation.x = Math.PI / 2;
      hub.position.z = floorZ + hubH / 2;
      const hubBase = add(new THREE.CylinderGeometry(hubR * 1.9 + .6, hubR * 1.9 + .8, .5, 48), traySeat);
      hubBase.rotation.x = Math.PI / 2;
      hubBase.position.z = floorZ + .25;
    }

    // Disc: thin ring, art on top, iridescent silver underneath. Centered even if it overflows.
    const discT = 1.2;
    const silver = new THREE.MeshPhysicalMaterial({
      color: 0xd9dde2, metalness: 1, roughness: .16, iridescence: 1, iridescenceIOR: 1.6,
      iridescenceThicknessMinimum: 180, iridescenceThicknessMaximum: 620, side: THREE.DoubleSide
    });
    const top = discArt
      ? new THREE.MeshPhysicalMaterial({map: discArt, roughness: .42, metalness: .05, clearcoat: .5, clearcoatRoughness: .2})
      : silver;
    const disc = new THREE.Group();
    disc.position.z = floorZ + .5 + discT / 2;
    const ringTop = add(new THREE.RingGeometry(holeR, discR, 128, 1), top, disc);
    ringTop.position.z = discT / 2;
    const ringBottom = add(new THREE.RingGeometry(holeR, discR, 128, 1), silver, disc);
    ringBottom.rotation.y = Math.PI;
    ringBottom.position.z = -discT / 2;
    const rimOuter = add(new THREE.CylinderGeometry(discR, discR, discT, 128, 1, true), silver, disc);
    rimOuter.rotation.x = Math.PI / 2;
    if (holeR > .05) {
      const rimInner = add(new THREE.CylinderGeometry(holeR, holeR, discT, 64, 1, true), silver, disc);
      rimInner.rotation.x = Math.PI / 2;
    }
    group.add(disc);

    // Lid: front cover hinged at the spine edge; rotates outward around the vertical axis.
    lidPivot = new THREE.Group();
    lidPivot.position.set(x0, 0, z1);
    add(quad([0, y0, .02], [W, y0, .02], [W, y1, .02], [0, y1, .02], uSpine, 1), art, lidPivot);
    add(quad([W, y0, -.02], [0, y0, -.02], [0, y1, -.02], [W, y1, -.02]), paper, lidPivot);
    add(box(.01, W, y0, y1, -t, -.04), shell, lidPivot);
    group.add(lidPivot);

    // Stand the case on the floor.
    group.position.y = H / 2;
    return group;
  }

  // Two framings: closed = front cover filling the view; open = eased in toward the disc.
  function cameraPose(d, f) {
    const tan = Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2);
    const aspect = Math.max(.5, camera.aspect || 1);
    const fit = (h, w) => Math.max(h, w / aspect) / (2 * tan);
    const closedDist = fit(d.H * 1.3, d.W * 1.6) + d.D;
    const openDist = closedDist * .9; // ease in a little; the disc stays at real scale
    const closedTarget = new THREE.Vector3(0, d.H * .45, d.D / 2); // a little low: clears the control bar
    const openTarget = new THREE.Vector3(-d.W * .12, d.H * .45, -d.D / 2);
    const closedDir = new THREE.Vector3(-.2, .1, 1).normalize();
    const openDir = new THREE.Vector3(.16, .32, 1).normalize();
    const target = closedTarget.lerp(openTarget, f);
    const dir = closedDir.lerp(openDir, f).normalize();
    const dist = closedDist + (openDist - closedDist) * f;
    return {target, position: dir.multiplyScalar(dist).add(target), dist: closedDist};
  }

  function applyAutoCamera(d) {
    const x = Math.min(1, Math.max(0, lidAngle / 110));
    const pose = cameraPose(d, x * x * (3 - 2 * x));
    controls.target.copy(pose.target);
    camera.position.copy(pose.position);
  }

  function fitCamera(d) {
    autoCamera = true;
    const {dist} = cameraPose(d, 0);
    camera.near = Math.max(.5, dist / 100);
    camera.far = dist * 20;
    camera.updateProjectionMatrix();
    controls.minDistance = Math.max(d.H, d.discD) * .35;
    controls.maxDistance = dist * 5;
    applyAutoCamera(d);
    controls.update();
  }

  function placeLights(d) {
    const s = Math.max(d.H, d.W, d.D);
    key.position.set(-s * .8, s * 2.2, s * 1.6);
    key.target.position.set(0, d.H / 2, 0);
    const cam = key.shadow.camera;
    cam.left = cam.bottom = -s * 1.5;
    cam.right = cam.top = s * 1.5;
    cam.near = s * .2;
    cam.far = s * 6;
    cam.updateProjectionMatrix();
    rim.position.set(s * 2, s * .8, -s * 1.5);
    floor.scale.set(s * 8, s * 8, 1);
  }

  async function rebuild() {
    dirty = false;
    rebuildTimer = 0;
    const token = ++buildToken;
    const d = readDims();
    let wrap = null, discArt = null;
    try { wrap = await makeWrapTexture(d); } catch (error) { console.warn('Plomcase 3D: wrap art unavailable', error); }
    try { discArt = await makeDiscTexture(); } catch (error) { console.warn('Plomcase 3D: disc art unavailable', error); }
    if (token !== buildToken) { wrap?.dispose(); discArt?.dispose(); return; }
    const sizeChanged = !dims || ['H', 'W', 'D'].some((k) => Math.abs(dims[k] - d[k]) > .01);
    disposeGroup(caseGroup);
    wrapTexture?.dispose();
    discTexture?.dispose();
    wrapTexture = wrap;
    discTexture = discArt;
    caseGroup = buildCase(d, wrap, discArt);
    scene.add(caseGroup);
    applyLid();
    const first = !dims;
    dims = d;
    if (sizeChanged) placeLights(d);
    if (first || autoCamera) fitCamera(d);
  }

  function scheduleRebuild() {
    if (!visible) { dirty = true; return; }
    clearTimeout(rebuildTimer);
    rebuildTimer = setTimeout(rebuild, REBUILD_DELAY);
  }

  function applyLid() {
    if (lidPivot) lidPivot.rotation.y = -THREE.MathUtils.degToRad(lidAngle);
  }

  function syncLidUi() {
    if (lidSlider && Number(lidSlider.value) !== Math.round(lidTarget)) lidSlider.value = String(Math.round(lidTarget));
    if (lidOutput) lidOutput.textContent = `${Math.round(lidTarget)}°`;
    if (lidButton) lidButton.textContent = lidTarget > 0 ? 'Close' : 'Open';
  }

  function resize() {
    const w = stage.clientWidth, h = stage.clientHeight;
    if (!w || !h) return;
    renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
    if (autoCamera && dims) fitCamera(dims);
  }
  const observer = new ResizeObserver(() => { if (visible) resize(); });
  observer.observe(stage);

  function loop() {
    frame = requestAnimationFrame(loop);
    if (Math.abs(lidTarget - lidAngle) > .05) {
      lidAngle += (lidTarget - lidAngle) * .16;
      if (Math.abs(lidTarget - lidAngle) <= .05) lidAngle = lidTarget;
      applyLid();
    }
    if (autoCamera && dims) applyAutoCamera(dims);
    controls.update();
    renderer.render(scene, camera);
  }

  lidSlider?.addEventListener('input', () => { lidTarget = Number(lidSlider.value) || 0; syncLidUi(); });
  lidButton?.addEventListener('click', () => { lidTarget = lidTarget > 0 ? 0 : DEFAULT_LID_OPEN; syncLidUi(); });
  controls.addEventListener('start', () => { autoCamera = false; });
  resetButton?.addEventListener('click', () => { if (dims) fitCamera(dims); });
  api.onChange?.(scheduleRebuild);
  syncLidUi();

  return {
    show() {
      if (visible) return;
      visible = true;
      resize();
      if (dirty) { clearTimeout(rebuildTimer); rebuild(); }
      cancelAnimationFrame(frame);
      loop();
    },
    hide() {
      visible = false;
      cancelAnimationFrame(frame);
      if (rebuildTimer) { clearTimeout(rebuildTimer); rebuildTimer = 0; dirty = true; }
    },
    setLid(angle, immediate = false) {
      lidTarget = num(angle, 0, 0, 150);
      if (immediate) { lidAngle = lidTarget; applyLid(); }
      syncLidUi();
    },
    whenReady() { return new Promise((resolve) => { const check = () => (caseGroup && !dirty ? resolve() : setTimeout(check, 50)); check(); }); }
  };
}
