(() => {
  const $ = (id) => document.getElementById(id);
  const ids = ['projectName','imageFit','faceHeight','spineWidth','bleed','dpi','cropMarks','spineDirection','fontFamily','spineFontSize','spineColor','textColor','spineFillMode','spineTextOutline','logoRotation'];
  const settingIds = ids.filter((id) => id !== 'projectName');
  const settingGroups = {
    artwork: ['imageFit'],
    layout: ['faceHeight','spineWidth','bleed','dpi','cropMarks'],
    theme: ['spineDirection','fontFamily','spineFontSize','spineColor','textColor','spineFillMode','spineTextOutline','logoRotation']
  };
  const builtInDefaults = {
    imageFit: 'cover', faceHeight: '120', spineWidth: '13', bleed: '0', dpi: '300', cropMarks: false,
    spineDirection: '90', fontFamily: "'Rubik', sans-serif", spineFontSize: '8', spineColor: '#ffffff',
    textColor: '#000000', spineFillMode: 'back-front', spineTextOutline: 'auto', logoRotation: '0'
  };
  const legacyDefaults = {
    imageFit: 'cover', faceHeight: '120', spineWidth: '12', bleed: '3', dpi: '300', cropMarks: true,
    spineDirection: '90', fontFamily: "'Rubik', sans-serif", spineFontSize: '6', spineColor: '#ffffff',
    textColor: '#000000', spineFillMode: 'back-front', spineTextOutline: 'auto', logoRotation: '0'
  };
  const legacyDefaultValues = {spineWidth: ['12', '8']};
  const defaultsStorageKey = 'plomcase.settingDefaults.v1';
  let settingDefaults = loadSettingDefaults();
  let settingOverrides = {};
  const files = {back: null, front: null, logo: null};
  const artworkEdgeCache = new Map();
  let spineEdgeColors = {back: null, front: null};
  let spinePaletteRevision = 0;
  let projects = [];
  let activeProjectId = null;
  let saveTimer;
  let toastTimer;
  let hasUnsavedChanges = false;
  let sgdbMode = 'grid';
  let sgdbGame = null;
  let sgdbPage = 0;
  let sgdbTotal = 0;
  let sgdbAssets = [];
  let sgdbRequestId = 0;
  const sgdbKeyStorage = 'plomcase.steamgriddb.apiKey';

  const dbPromise = new Promise((resolve, reject) => {
    if (!('indexedDB' in window)) { reject(new Error('Local project storage is unavailable')); return; }
    const req = indexedDB.open('casecraft-library', 1);
    req.onupgradeneeded = () => req.result.createObjectStore('projects', {keyPath: 'id'});
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error || new Error('Could not open local project storage'));
  });

  function normalizeSettingValue(id, value, fallback = builtInDefaults[id]) {
    if (id === 'cropMarks') return typeof value === 'boolean' ? value : fallback;
    if (id === 'spineColor' || id === 'textColor') return safeColor(value, fallback);
    const choices = {
      imageFit: ['cover', 'contain'], dpi: ['150', '300', '600'], spineDirection: ['90', '-90'],
      fontFamily: ["'Rubik', sans-serif", "Impact, Haettenschweiler, 'Arial Narrow Bold', sans-serif"],
      spineFillMode: ['back-front','front-back','dark-blend','light-blend','back-edge','front-edge','center-mix','solid'],
      spineTextOutline: ['auto','none','white','black']
    };
    if (choices[id]) return choices[id].includes(String(value)) ? String(value) : fallback;
    const ranges = {faceHeight: [40,350], spineWidth: [3,60], bleed: [0,10], spineFontSize: [2,24], logoRotation: [-180,180]};
    if (ranges[id]) {
      const number = Number(value);
      return Number.isFinite(number) && number >= ranges[id][0] && number <= ranges[id][1] ? String(number) : fallback;
    }
    return fallback;
  }

  function loadSettingDefaults() {
    let saved = {};
    try {
      const parsed = JSON.parse(localStorage.getItem(defaultsStorageKey) || '{}');
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) saved = parsed;
    } catch {}
    return Object.fromEntries(settingIds.map((id) => [id, normalizeSettingValue(id, saved[id], builtInDefaults[id])]));
  }

  function renderSettingSources() {
    settingIds.forEach((id) => {
      const source = $(`${id}Source`);
      if (source) source.textContent = settingOverrides[id] ? '(override)' : '(default)';
    });
  }

  function setControlValue(id, value) {
    const control = $(id);
    if (control.type === 'checkbox') control.checked = Boolean(value);
    else control.value = value;
  }

  function applySharedDefaultsToControls() {
    settingIds.forEach((id) => setControlValue(id, settingDefaults[id]));
    settingOverrides = {};
    renderSettingSources();
  }

  function readState() {
    const values = {};
    ids.forEach((id) => { const el = $(id); values[id] = el.type === 'checkbox' ? el.checked : el.value; });
    return {...values, settingOverrides: {...settingOverrides}, backImage: files.back, frontImage: files.front, logoImage: files.logo, version: 1};
  }

  function confirmDiscard(action) {
    if (!hasUnsavedChanges) return true;
    return confirm(`You have unsaved changes. ${action} without saving?`);
  }

  function normalizeState(state = {}) {
    const normalized = {...state};
    if ((!normalized.projectName || normalized.projectName === 'Untitled game case') && normalized.spineTitle) normalized.projectName = normalized.spineTitle;
    return normalized;
  }

  function inferSettingOverrides(state = {}) {
    const stored = state.settingOverrides && typeof state.settingOverrides === 'object' ? state.settingOverrides : null;
    if (stored) return Object.fromEntries(settingIds.filter((id) => stored[id] === true && state[id] !== undefined).map((id) => [id, true]));
    return Object.fromEntries(settingIds.filter((id) => {
      if (state[id] === undefined) return false;
      const saved = normalizeSettingValue(id, state[id], legacyDefaults[id]);
      const legacyValues = legacyDefaultValues[id] || [legacyDefaults[id]];
      return !legacyValues.some((legacy) => (typeof saved === 'string' && saved.startsWith('#'))
        ? saved.toLowerCase() === String(legacy).toLowerCase()
        : saved === legacy);
    }).map((id) => [id, true]));
  }

  async function applyState(state = {}) {
    state = normalizeState(state);
    const restoredLogo = await normalizeLogo(safeImage(state.logoImage));
    $('projectName').value = state.projectName || '';
    settingOverrides = inferSettingOverrides(state);
    settingIds.forEach((id) => {
      const value = settingOverrides[id]
        ? normalizeSettingValue(id, state[id], settingDefaults[id])
        : settingDefaults[id];
      setControlValue(id, value);
    });
    renderSettingSources();
    files.back = safeImage(state.backImage);
    files.front = safeImage(state.frontImage);
    files.logo = restoredLogo;
    $('backFileName').textContent = state.backFileName || (files.back ? 'Image ready' : 'Add image');
    $('frontFileName').textContent = state.frontFileName || (files.front ? 'Image ready' : 'Add image');
    $('logoFileName').textContent = state.logoFileName || (files.logo ? 'Logo ready' : 'Add image');
    $('removeSpineLogo').hidden = !files.logo;
    hasUnsavedChanges = false;
    refreshSpinePalette();
    update();
  }

  function esc(value) {
    return String(value).replace(/[&<>"']/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&apos;'}[c]));
  }

  function safeColor(value, fallback) {
    return /^#[0-9a-f]{6}$/i.test(String(value || '')) ? value : fallback;
  }

  function sampleArtworkEdges(data) {
    if (!data) return Promise.resolve(null);
    if (artworkEdgeCache.has(data)) return artworkEdgeCache.get(data);
    const sampled = new Promise((resolve) => {
      const image = new Image();
      image.onload = () => {
        try {
          const canvas = document.createElement('canvas');
          canvas.width = 64;
          canvas.height = 96;
          const context = canvas.getContext('2d', {willReadFrequently: true});
          context.drawImage(image, 0, 0, canvas.width, canvas.height);
          const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
          const edgeColor = (edge) => {
            const buckets = new Map();
            const startX = edge === 'left' ? 0 : canvas.width - 4;
            for (let y = 1; y < canvas.height; y += 2) {
              for (let x = startX; x < startX + 4; x += 1) {
                const index = (y * canvas.width + x) * 4;
                const alpha = pixels[index + 3] / 255;
                if (alpha < .18) continue;
                const rgb = [0, 1, 2].map((channel) => Math.round(pixels[index + channel] * alpha + 255 * (1 - alpha)));
                const key = rgb.map((channel) => Math.round(channel / 32)).join(',');
                const bucket = buckets.get(key) || {count: 0, rgb: [0, 0, 0]};
                bucket.count += 1;
                rgb.forEach((channel, i) => { bucket.rgb[i] += channel; });
                buckets.set(key, bucket);
              }
            }
            const winner = [...buckets.values()].sort((a, b) => b.count - a.count)[0];
            if (!winner) return null;
            return `#${winner.rgb.map((channel) => Math.round(channel / winner.count).toString(16).padStart(2, '0')).join('')}`;
          };
          resolve({left: edgeColor('left'), right: edgeColor('right')});
        } catch {
          resolve(null);
        }
      };
      image.onerror = () => resolve(null);
      image.src = data;
    });
    artworkEdgeCache.set(data, sampled);
    if (artworkEdgeCache.size > 2) artworkEdgeCache.delete(artworkEdgeCache.keys().next().value);
    return sampled;
  }

  function refreshSpinePalette() {
    const revision = ++spinePaletteRevision;
    spineEdgeColors = {back: null, front: null};
    Promise.all([sampleArtworkEdges(files.back), sampleArtworkEdges(files.front)]).then(([back, front]) => {
      if (revision !== spinePaletteRevision) return;
      spineEdgeColors = {back: back?.right || null, front: front?.left || null};
      update();
    });
  }

  function mixColor(start, end, amount) {
    const a = start.match(/[a-f\d]{2}/gi).map((value) => parseInt(value, 16));
    const b = end.match(/[a-f\d]{2}/gi).map((value) => parseInt(value, 16));
    return `#${a.map((value, i) => Math.round(value * (1 - amount) + b[i] * amount).toString(16).padStart(2, '0')).join('')}`;
  }

  function relativeLuminance(color) {
    const channels = color.match(/[a-f\d]{2}/gi).map((value) => parseInt(value, 16) / 255).map((value) => value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4);
    return channels[0] * .2126 + channels[1] * .7152 + channels[2] * .0722;
  }

  function contrastRatio(first, second) {
    const a = relativeLuminance(first), b = relativeLuminance(second);
    return (Math.max(a, b) + .05) / (Math.min(a, b) + .05);
  }

  function textOutline(mode, ink, backgrounds) {
    if (mode === 'none') return '';
    if (mode === 'black') return '#000000';
    if (mode === 'white') return '#ffffff';
    if (backgrounds.every((background) => contrastRatio(ink, background) >= 4.5)) return '';
    return ['#000000', '#ffffff'].sort((a, b) => {
      const score = (outline) => Math.min(contrastRatio(outline, ink), ...backgrounds.map((background) => contrastRatio(outline, background)));
      return score(b) - score(a);
    })[0];
  }

  function safeImage(value) {
    return typeof value === 'string' && /^data:image\/(?:png|jpeg|webp|gif|avif|x-icon|vnd\.microsoft\.icon|svg\+xml);base64,[a-z0-9+/]+=*$/i.test(value) ? value : null;
  }

  function clampNumber(id, fallback, min, max) {
    const n = Number($(id).value);
    if (!Number.isFinite(n)) return fallback;
    return Math.min(max, Math.max(min, n));
  }

  function imageMarkup(data, x, y, w, h, fit, label, background) {
    const out = [`<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="${background}"/>`];
    if (data) {
      const aspect = fit === 'contain' ? 'xMidYMid meet' : 'xMidYMid slice';
      out.push(`<image href="${data}" x="${x}" y="${y}" width="${w}" height="${h}" preserveAspectRatio="${aspect}"/>`);
    } else {
      out.push(`<rect x="${x+2}" y="${y+2}" width="${w-4}" height="${h-4}" fill="none" stroke="#ffffff" stroke-opacity=".22" stroke-dasharray="2 2"/>`);
      out.push(`<text x="${x+w/2}" y="${y+h/2}" text-anchor="middle" fill="#ffffff" fill-opacity=".7" font-family="Arial,sans-serif" font-size="4">${esc(label)}</text>`);
      out.push(`<text x="${x+w/2}" y="${y+h/2+6}" text-anchor="middle" fill="#ffffff" fill-opacity=".45" font-family="Arial,sans-serif" font-size="2.5">2:3 ART</text>`);
    }
    return out.join('');
  }

  function buildSvg(s = readState(), includeGuides = false) {
    const faceH = clampNumber('faceHeight', 120, 40, 350);
    const faceW = faceH * 2 / 3;
    const spineW = clampNumber('spineWidth', 13, 3, 60);
    const bleed = clampNumber('bleed', 0, 0, 10);
    const trimW = faceW * 2 + spineW;
    const totalW = trimW + bleed * 2;
    const totalH = faceH + bleed * 2;
    const backX = bleed, spineX = bleed + faceW, frontX = spineX + spineW;
    const theme = safeColor(s.spineColor, '#ffffff');
    const ink = safeColor(s.textColor, '#000000');
    const fillMode = s.spineFillMode || 'solid';
    const backEdge = safeColor(spineEdgeColors.back, theme);
    const frontEdge = safeColor(spineEdgeColors.front, theme);
    let spinePaint = theme;
    let spineGradient = '';
    let titleBackgrounds = [theme, theme];
    if (fillMode === 'back-edge') {
      spinePaint = backEdge;
      titleBackgrounds = [backEdge, backEdge];
    } else if (fillMode === 'front-edge') {
      spinePaint = frontEdge;
      titleBackgrounds = [frontEdge, frontEdge];
    } else if (fillMode === 'center-mix') {
      spinePaint = mixColor(backEdge, frontEdge, .5);
      titleBackgrounds = [spinePaint, spinePaint];
    } else if (['back-front', 'front-back', 'dark-blend', 'light-blend'].includes(fillMode)) {
      let first = fillMode === 'front-back' ? frontEdge : backEdge;
      let last = fillMode === 'front-back' ? backEdge : frontEdge;
      if (fillMode === 'dark-blend') {
        first = mixColor(first, '#10151b', .38);
        last = mixColor(last, '#10151b', .38);
      } else if (fillMode === 'light-blend') {
        first = mixColor(first, '#ffffff', .4);
        last = mixColor(last, '#ffffff', .4);
      }
      titleBackgrounds = [first, last];
      spinePaint = 'url(#spineFillGradient)';
      spineGradient = `<defs><linearGradient id="spineFillGradient" x1="${spineX}" y1="${bleed}" x2="${spineX+spineW}" y2="${bleed}" gradientUnits="userSpaceOnUse"><stop offset="0%" stop-color="${first}"/><stop offset="100%" stop-color="${last}"/></linearGradient></defs>`;
    }
    const outline = textOutline(s.spineTextOutline || 'auto', ink, titleBackgrounds);
    const outlineWidth = outline ? .55 : 0;
    const font = esc(s.fontFamily || "'Rubik', sans-serif");
    const fit = s.imageFit || 'cover';
    const requestedFontSize = clampNumber('spineFontSize', 8, 2, 24);
    let spineFit = {requested: requestedFontSize, effective: requestedFontSize, reason: ''};
    let parts = [`<svg xmlns="http://www.w3.org/2000/svg" width="${totalW}mm" height="${totalH}mm" viewBox="0 0 ${totalW} ${totalH}">`,
      spineGradient,
      `<rect width="${totalW}" height="${totalH}" fill="${theme}"/>`,
      imageMarkup(s.backImage, 0, 0, faceW + bleed, faceH + 2 * bleed, fit, 'Back cover artwork', '#343c35'),
      imageMarkup(s.frontImage, frontX, 0, faceW + bleed, faceH + 2 * bleed, fit, 'Front cover artwork', '#30382f'),
      `<rect x="${spineX}" y="${bleed}" width="${spineW}" height="${faceH}" fill="${spinePaint}"/>`];

    const spine = String(s.projectName || '').trim();
    const logoLength = Math.min(20, Math.max(10, faceH * .18));
    const spineCenterY = bleed + faceH / 2;
    if (spine) {
      const dir = Number(s.spineDirection) === 90 ? 90 : -90;
      const chars = Array.from(spine);
      const units = chars.reduce((sum, ch) => sum + (ch === ' ' ? .36 : /[il.,'!|]/.test(ch) ? .34 : /[MW@#]/.test(ch) ? .86 : .63), 0);
      const letterSpacing = .15;
      const availableLength = Math.max(10, faceH - 16);
      const widthLimit = Math.max(1, (spineW - 2 - outlineWidth * 2) / .82);
      const lengthLimit = Math.max(1, (availableLength - Math.max(0, chars.length - 1) * letterSpacing) / Math.max(.1, units));
      const fontSize = Math.max(1, Math.min(requestedFontSize, widthLimit, lengthLimit));
      const reason = widthLimit < requestedFontSize && widthLimit <= lengthLimit ? 'spine width' : 'title length';
      spineFit = {requested: requestedFontSize, effective: fontSize, reason: fontSize < requestedFontSize - .05 ? reason : ''};
      const estimated = units * fontSize + Math.max(0, chars.length - 1) * letterSpacing;
      const squeeze = estimated > availableLength ? ` textLength="${availableLength}" lengthAdjust="spacingAndGlyphs"` : '';
      const outlineAttrs = outline ? ` stroke="${outline}" stroke-width="${outlineWidth}" stroke-linejoin="round" paint-order="stroke"` : '';
      parts.push(`<text x="${spineX+spineW/2}" y="${spineCenterY}" transform="rotate(${dir} ${spineX+spineW/2} ${spineCenterY})" text-anchor="middle" dominant-baseline="central" fill="${ink}"${outlineAttrs} font-family="${font}" font-size="${fontSize}" font-weight="700" letter-spacing=".15"${squeeze}>${esc(spine)}</text>`);
    } else {
      const hintSize = Math.min(2.5, spineW * .55);
      parts.push(`<text x="${spineX+spineW/2}" y="${spineCenterY}" transform="rotate(90 ${spineX+spineW/2} ${spineCenterY})" text-anchor="middle" fill="${ink}" fill-opacity=".45" font-family="${font}" font-size="${hintSize}">PROJECT NAME</text>`);
    }
    if (s.logoImage) {
      const logoCenterX = spineX + spineW / 2;
      const rotation = Number(s.logoRotation);
      const dir = Number.isFinite(rotation) ? Math.max(-180, Math.min(180, rotation)) : 0;
      const radians = dir * Math.PI / 180;
      const cos = Math.abs(Math.cos(radians)), sin = Math.abs(Math.sin(radians));
      const logoCross = Math.max(1, spineW - 2);
      const vertical = sin > cos;
      let boxW = vertical ? logoLength : logoCross;
      let boxH = vertical ? logoCross : logoLength;
      const projectedW = boxW * cos + boxH * sin;
      const projectedH = boxW * sin + boxH * cos;
      const scale = Math.min(1, logoCross / Math.max(.001, projectedW), logoLength / Math.max(.001, projectedH));
      boxW *= scale;
      boxH *= scale;
      const finalProjectedH = boxW * sin + boxH * cos;
      const bottomInset = 1;
      const logoCenterY = bleed + faceH - bottomInset - finalProjectedH / 2;
      const imageAlign = !vertical && cos > .999 && dir > 90 ? 'xMidYMin meet' : (!vertical && cos > .999 ? 'xMidYMax meet' : 'xMidYMid meet');
      parts.push(`<g transform="rotate(${dir} ${logoCenterX} ${logoCenterY})"><image href="${s.logoImage}" x="${logoCenterX-boxW/2}" y="${logoCenterY-boxH/2}" width="${boxW}" height="${boxH}" preserveAspectRatio="${imageAlign}"/></g>`);
    }

    if (s.cropMarks && bleed > 0) {
      const mark = Math.min(2.5, bleed * .8), gap = Math.min(.6, bleed * .2), x1=bleed, x2=bleed+trimW, y1=bleed, y2=bleed+faceH;
      const paths = [
        `M${x1} ${y1-mark-gap}v${mark} M${x1-mark-gap} ${y1}h${mark}`,
        `M${x2} ${y1-mark-gap}v${mark} M${x2+gap} ${y1}h${-mark}`,
        `M${x1} ${y2+gap}v${mark} M${x1-mark-gap} ${y2}h${mark}`,
        `M${x2} ${y2+gap}v${mark} M${x2+gap} ${y2}h${-mark}`
      ];
      parts.push(`<path d="${paths.join(' ')}" fill="none" stroke="#111" stroke-width=".18"/>`);
    }
    if (includeGuides && $('toggleGuides').getAttribute('aria-pressed') === 'true') {
      parts.push(`<rect x="${backX}" y="${bleed}" width="${faceW}" height="${faceH}" fill="none" stroke="#c5df60" stroke-width=".25" stroke-dasharray="2 1"/>`);
      parts.push(`<rect x="${spineX}" y="${bleed}" width="${spineW}" height="${faceH}" fill="none" stroke="#c5df60" stroke-width=".25" stroke-dasharray="2 1"/>`);
      parts.push(`<rect x="${frontX}" y="${bleed}" width="${faceW}" height="${faceH}" fill="none" stroke="#c5df60" stroke-width=".25" stroke-dasharray="2 1"/>`);
    }
    parts.push('</svg>');
    return {svg: parts.join(''), width: totalW, height: totalH, trimW, faceW, faceH, spineW, bleed, spineFit};
  }

  function update() {
    const {svg, width, height, faceW, spineW, spineFit} = buildSvg(undefined, true);
    $('preview').outerHTML = svg.replace('<svg ', '<svg id="preview" role="img" aria-label="Printable back, spine, and front cover preview" ');
    const ready = Boolean(files.back || files.front || $('projectName').value.trim());
    $('previewStage').classList.toggle('has-art', ready);
    $('emptyHint').style.display = ready ? 'none' : '';
    const trimWidth = (faceW * 2 + spineW).toFixed(1);
    $('exportDimensions').textContent = `${width.toFixed(1)} × ${height.toFixed(1)} mm with bleed · ${trimWidth} mm trim width`;
    $('logoRotationValue').textContent = `${Number($('logoRotation').value) < 0 ? '−' : ''}${Math.abs(Number($('logoRotation').value))}°`;
    const title = $('projectName').value.trim();
    const search = new URL('https://www.steamgriddb.com/search/grids');
    search.searchParams.set('term', title);
    $('steamGridSearch').href = search.toString();
    $('steamSearchTerm').textContent = title || 'game title';
    const fitNote = $('spineFitNote');
    fitNote.classList.remove('adjusted','tight');
    if (!title) fitNote.textContent = 'Enter a project name to check fit.';
    else if (spineFit.reason) {
      fitNote.textContent = `Auto-fit to ${spineFit.effective.toFixed(1)} mm for ${spineFit.reason}.`;
      fitNote.classList.add('adjusted');
      if (spineFit.effective < spineFit.requested * .7) fitNote.classList.add('tight');
    } else fitNote.textContent = `Fits at ${spineFit.effective.toFixed(1)} mm.`;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(renderProjectList, 700);
  }

  function toast(message) {
    $('toast').textContent = message;
    $('toast').classList.add('show');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => $('toast').classList.remove('show'), 2400);
  }

  function imageData(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result);
      reader.onerror = () => reject(reader.error || new Error('Could not read image'));
      reader.readAsDataURL(file);
    });
  }

  function normalizeLogo(data) {
    if (typeof data !== 'string' || !/^data:image\/(?:x-icon|vnd\.microsoft\.icon);base64,/i.test(data)) return Promise.resolve(data);
    return new Promise((resolve) => {
      const image = new Image();
      image.onload = () => {
        try {
          const canvas = document.createElement('canvas');
          canvas.width = Math.max(1, image.naturalWidth || image.width);
          canvas.height = Math.max(1, image.naturalHeight || image.height);
          const context = canvas.getContext('2d');
          if (!context) { resolve(data); return; }
          context.drawImage(image, 0, 0);
          resolve(canvas.toDataURL('image/png'));
        } catch {
          resolve(data);
        }
      };
      image.onerror = () => resolve(data);
      image.src = data;
    });
  }

  async function addImage(which, file) {
    if (!file) return;
    const isIcoFile = /\.ico$/i.test(file.name || '');
    if (!file.type.startsWith('image/') && !isIcoFile) { toast('Choose an image file.'); return; }
    if (file.size > 40 * 1024 * 1024) { toast('Please choose an image under 40 MB.'); return; }
    if (files[which] && !confirm(`Replace the current ${which === 'logo' ? 'spine logo' : `${which} cover`} with this image?`)) return;
    try {
      const iconMimeKnown = /^image\/(?:x-icon|vnd\.microsoft\.icon)$/i.test(file.type);
      const source = isIcoFile && !iconMimeKnown ? new Blob([file], {type: 'image/x-icon'}) : file;
      const data = await imageData(source);
      files[which] = which === 'logo' ? await normalizeLogo(data) : data;
      $(`${which}FileName`).textContent = file.name;
      if (which === 'logo') $('removeSpineLogo').hidden = false;
      hasUnsavedChanges = true;
      if (which === 'back' || which === 'front') refreshSpinePalette();
      else update();
    } catch { toast('Could not load that image.'); }
  }

  function safeFilename(name) {
    return (name || 'game-case').trim().replace(/[^a-z0-9-_]+/gi, '-').replace(/^-|-$/g, '').toLowerCase() || 'game-case';
  }

  function downloadBlob(blob, name) {
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a'); a.href = url; a.download = name; document.body.appendChild(a); a.click(); a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1500);
  }

  function crc32(bytes) {
    let crc = 0xffffffff;
    for (let i = 0; i < bytes.length; i++) {
      crc ^= bytes[i];
      for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
    }
    return (crc ^ 0xffffffff) >>> 0;
  }

  async function pngWithDpi(blob, dpi) {
    const input = new Uint8Array(await blob.arrayBuffer());
    const signature = input.subarray(0, 8);
    const chunks = [signature];
    const metadata = new Uint8Array(21);
    const view = new DataView(metadata.buffer);
    view.setUint32(0, 9, false);
    metadata.set([112, 72, 89, 115], 4); // pHYs
    const pixelsPerMeter = Math.round(dpi / 0.0254);
    view.setUint32(8, pixelsPerMeter, false);
    view.setUint32(12, pixelsPerMeter, false);
    metadata[16] = 1; // Pixels per meter
    view.setUint32(17, crc32(metadata.subarray(4, 17)), false);
    let pos = 8, inserted = false;
    while (pos + 12 <= input.length) {
      const length = new DataView(input.buffer, input.byteOffset + pos, 4).getUint32(0, false);
      const end = pos + length + 12;
      if (end > input.length) throw new Error('Invalid PNG data');
      const type = String.fromCharCode(...input.subarray(pos + 4, pos + 8));
      if (type !== 'pHYs') chunks.push(input.subarray(pos, end));
      if (type === 'IHDR' && !inserted) { chunks.push(metadata); inserted = true; }
      pos = end;
      if (type === 'IEND') break;
    }
    if (!inserted) throw new Error('Could not set PNG resolution metadata');
    return new Blob(chunks, {type: 'image/png'});
  }

  $('downloadSvg').addEventListener('click', () => {
    const {svg} = buildSvg();
    downloadBlob(new Blob([svg], {type: 'image/svg+xml;charset=utf-8'}), `${safeFilename($('projectName').value)}.svg`);
    toast('SVG downloaded.');
  });

  $('downloadPng').addEventListener('click', () => {
    const drawing = buildSvg();
    const dpi = Number($('dpi').value) || 300;
    const widthPx = Math.round(drawing.width / 25.4 * dpi);
    const heightPx = Math.round(drawing.height / 25.4 * dpi);
    const svgBlob = new Blob([drawing.svg], {type: 'image/svg+xml;charset=utf-8'});
    const svgUrl = URL.createObjectURL(svgBlob);
    const img = new Image();
    img.onload = () => {
      const canvas = document.createElement('canvas'); canvas.width = widthPx; canvas.height = heightPx;
      const ctx = canvas.getContext('2d', {alpha: false});
      ctx.fillStyle = '#fff'; ctx.fillRect(0, 0, widthPx, heightPx); ctx.drawImage(img, 0, 0, widthPx, heightPx);
      canvas.toBlob(async (blob) => {
        URL.revokeObjectURL(svgUrl);
        if (blob) {
          try {
            const printBlob = await pngWithDpi(blob, dpi);
            downloadBlob(printBlob, `${safeFilename($('projectName').value)}-${dpi}dpi.png`);
            toast(`Print PNG exported at ${dpi} DPI.`);
          } catch { toast('PNG metadata could not be written. Try the SVG export.'); }
        }
        else toast('PNG export failed. Try a smaller image or 300 DPI.');
      }, 'image/png');
    };
    img.onerror = () => { URL.revokeObjectURL(svgUrl); toast('Could not render the print PNG. Try SVG export instead.'); };
    img.src = svgUrl;
  });

  function idbRequest(mode, action) {
    return dbPromise.then((db) => new Promise((resolve, reject) => {
      const tx = db.transaction('projects', mode);
      const req = action(tx.objectStore('projects'));
      let result;
      req.onsuccess = () => { result = req.result; };
      req.onerror = () => reject(req.error || new Error('Local save failed'));
      tx.oncomplete = () => resolve(result);
      tx.onerror = () => reject(tx.error || new Error('Local save failed'));
      tx.onabort = () => reject(tx.error || new Error('Local save was not committed'));
    }));
  }

  function clearSavedOverrideGroup(id, group) {
    return dbPromise.then((db) => new Promise((resolve, reject) => {
      const tx = db.transaction('projects', 'readwrite');
      const store = tx.objectStore('projects');
      const req = store.get(id);
      let updated = false;
      req.onsuccess = () => {
        if (!req.result) return;
        const record = req.result;
        const overrides = inferSettingOverrides(record.state);
        settingGroups[group].forEach((field) => { delete overrides[field]; });
        record.state = {...record.state, settingOverrides: overrides};
        record.updatedAt = Date.now();
        store.put(record);
        updated = true;
      };
      req.onerror = () => reject(req.error || new Error('Could not update project defaults'));
      tx.oncomplete = () => resolve(updated);
      tx.onerror = () => reject(tx.error || new Error('Could not update project defaults'));
      tx.onabort = () => reject(tx.error || new Error('Project defaults were not saved'));
    }));
  }

  async function refreshDirtyStatusFromSavedProject() {
    if (!activeProjectId) return;
    try {
      const record = await idbRequest('readonly', (store) => store.get(activeProjectId));
      if (!record) return;
      const saved = normalizeState(record.state);
      const current = readState();
      const savedLogo = await normalizeLogo(safeImage(saved.logoImage));
      let dirty = current.projectName !== (saved.projectName || '')
        || current.backImage !== safeImage(saved.backImage)
        || current.frontImage !== safeImage(saved.frontImage)
        || current.logoImage !== savedLogo;
      const savedOverrides = inferSettingOverrides(saved);
      for (const id of settingIds) {
        if (Boolean(current.settingOverrides[id]) !== Boolean(savedOverrides[id])) { dirty = true; break; }
        const currentValue = normalizeSettingValue(id, current[id], settingDefaults[id]);
        const savedValue = savedOverrides[id] ? normalizeSettingValue(id, saved[id], settingDefaults[id]) : settingDefaults[id];
        const equal = (typeof currentValue === 'string' && currentValue.startsWith('#'))
          ? currentValue.toLowerCase() === String(savedValue).toLowerCase()
          : currentValue === savedValue;
        if (!equal) { dirty = true; break; }
      }
      hasUnsavedChanges = dirty;
    } catch {}
  }

  async function saveSettingsAsDefault(group) {
    const fields = settingGroups[group];
    if (!fields) return;
    const nextDefaults = {...settingDefaults};
    fields.forEach((id) => {
      const control = $(id);
      nextDefaults[id] = normalizeSettingValue(id, control.type === 'checkbox' ? control.checked : control.value, settingDefaults[id]);
    });
    let previousStorage;
    try {
      previousStorage = localStorage.getItem(defaultsStorageKey);
      localStorage.setItem(defaultsStorageKey, JSON.stringify(nextDefaults));
      if (activeProjectId) await clearSavedOverrideGroup(activeProjectId, group);
    } catch {
      try {
        if (previousStorage === null) localStorage.removeItem(defaultsStorageKey);
        else if (previousStorage !== undefined) localStorage.setItem(defaultsStorageKey, previousStorage);
      } catch {}
      toast('Could not save these shared defaults.');
      return;
    }
    settingDefaults = nextDefaults;
    settingOverrides = {...settingOverrides};
    fields.forEach((id) => {
      delete settingOverrides[id];
      setControlValue(id, settingDefaults[id]);
    });
    renderSettingSources();
    update();
    if (activeProjectId) {
      await refreshDirtyStatusFromSavedProject();
      await listProjects();
    }
    toast('Section saved as the shared default.');
  }

  function resetSettingsToDefault(group) {
    const fields = settingGroups[group];
    if (!fields) return;
    fields.forEach((id) => {
      setControlValue(id, settingDefaults[id]);
      delete settingOverrides[id];
    });
    renderSettingSources();
    hasUnsavedChanges = true;
    update();
    toast('Section now uses the shared defaults. Save the project to keep this change.');
  }

  async function listProjects() {
    try { projects = (await idbRequest('readonly', (store) => store.getAll())) || []; }
    catch { projects = []; }
    projects.sort((a,b) => b.updatedAt - a.updatedAt);
    renderProjectList();
  }

  async function renderProjectList() {
    if (!('indexedDB' in window)) return;
    const container = $('projectList');
    if (!projects.length) {
      container.innerHTML = '<div class="library-empty">Saved case designs will appear here.</div>';
      $('projectCount').textContent = '0'; return;
    }
    $('projectCount').textContent = String(projects.length);
    container.innerHTML = projects.map((p) => {
      const state = normalizeState(p.state);
      const img = safeImage(state.frontImage) || safeImage(state.backImage);
      const thumb = img ? `<img src="${img}" alt="">` : '▤';
      const name = state.projectName || 'Untitled';
      const logoBadge = safeImage(state.logoImage) ? '<span class="project-logo-badge" title="Spine icon saved" aria-label="Spine icon saved">✦</span>' : '';
      return `<article class="project-card" data-card="${esc(p.id)}"><div class="project-thumb">${thumb}${logoBadge}</div><div class="project-info"><b title="${esc(name)}">${esc(name)}</b><small>${new Date(p.updatedAt).toLocaleDateString()}</small></div><div class="project-card-actions"><button data-action="load" title="Open project" aria-label="Open project">Open</button><button data-action="delete" title="Delete project" aria-label="Delete project">×</button></div></article>`;
    }).join('');
  }

  async function saveProject() {
    const state = readState();
    const record = {id: activeProjectId || (crypto.randomUUID ? crypto.randomUUID() : String(Date.now())), updatedAt: Date.now(), state};
    try {
      await idbRequest('readwrite', (store) => store.put(record));
      activeProjectId = record.id;
      hasUnsavedChanges = false;
      await listProjects();
      toast('Project saved in this browser.');
    } catch (err) {
      downloadBlob(new Blob([JSON.stringify(state, null, 2)], {type: 'application/json'}), `${safeFilename(state.projectName)}.plomcase.json`);
      hasUnsavedChanges = false;
      toast('Browser storage unavailable; downloaded a project backup.');
    }
  }

  function exportProjectFile() {
    const state = readState();
    downloadBlob(new Blob([JSON.stringify(state, null, 2)], {type: 'application/json'}), `${safeFilename(state.projectName)}.plomcase.json`);
    hasUnsavedChanges = false;
    toast('Project file exported.');
  }

  $('saveProject').addEventListener('click', saveProject);
  $('exportProject').addEventListener('click', exportProjectFile);
  $('newProject').addEventListener('click', async () => {
    if (hasUnsavedChanges ? !confirmDiscard('start a new case') : !confirm('Start a new case?')) return;
    files.back = files.front = files.logo = null;
    activeProjectId = null;
    $('backFileName').textContent = $('frontFileName').textContent = 'Add image';
    $('logoFileName').textContent = 'Add image';
    $('removeSpineLogo').hidden = true;
    await applyState({projectName:'',settingOverrides:{}});
  });
  $('importProject').addEventListener('click', () => $('projectFile').click());
  $('projectFile').addEventListener('change', async (e) => {
    const file = e.target.files[0]; e.target.value = '';
    if (!file) return;
    if (!confirmDiscard('open the selected project')) return;
    try { activeProjectId = null; await applyState(JSON.parse(await file.text())); toast('Project opened.'); }
    catch { toast('That project file could not be opened.'); }
  });

  $('projectList').addEventListener('click', async (e) => {
    const button = e.target.closest('button[data-action]'); if (!button) return;
    const card = button.closest('[data-card]'); const record = projects.find((p) => p.id === card.dataset.card); if (!record) return;
    if (button.dataset.action === 'load') {
      if (!confirmDiscard('load this saved project')) return;
      activeProjectId = record.id; await applyState(record.state); toast('Project loaded.');
    }
    if (button.dataset.action === 'delete') {
      if (!confirm(`Delete “${record.state.projectName || 'Untitled'}” from this browser?`)) return;
      try { await idbRequest('readwrite', (store) => store.delete(record.id)); if (activeProjectId === record.id) activeProjectId = null; await listProjects(); toast('Project deleted.'); }
      catch { toast('Could not delete that project.'); }
    }
  });

  document.querySelectorAll('[data-default-action]').forEach((button) => {
    button.addEventListener('click', () => {
      const {defaultAction, defaultGroup} = button.dataset;
      if (defaultAction === 'save') saveSettingsAsDefault(defaultGroup);
      if (defaultAction === 'reset') resetSettingsToDefault(defaultGroup);
    });
  });

  $('toggleGuides').addEventListener('click', (e) => {
    const pressed = e.currentTarget.getAttribute('aria-pressed') !== 'true';
    e.currentTarget.setAttribute('aria-pressed', String(pressed)); update();
  });
  async function chooseImage(which, input) {
    await addImage(which, input.files[0]);
    input.value = '';
  }
  $('backFile').addEventListener('change', (e) => chooseImage('back', e.target));
  $('frontFile').addEventListener('change', (e) => chooseImage('front', e.target));
  $('logoFile').addEventListener('change', (e) => chooseImage('logo', e.target));
  $('removeSpineLogo').addEventListener('click', () => {
    if (!files.logo || !confirm('Remove the spine logo from this case?')) return;
    files.logo = null;
    $('logoFileName').textContent = 'Add image';
    $('removeSpineLogo').hidden = true;
    hasUnsavedChanges = true;
    update();
  });
  document.querySelectorAll('.upload-card').forEach((card) => {
    const highlight = (e) => { e.preventDefault(); e.stopPropagation(); card.classList.add('dragging'); };
    card.addEventListener('dragenter', highlight);
    card.addEventListener('dragover', highlight);
    card.addEventListener('dragleave', (e) => {
      e.preventDefault();
      if (!e.relatedTarget || !card.contains(e.relatedTarget)) card.classList.remove('dragging');
    });
    card.addEventListener('drop', (e) => {
      e.preventDefault(); e.stopPropagation(); card.classList.remove('dragging');
      const file = Array.from(e.dataTransfer.files || []).find((item) => item.type.startsWith('image/'));
      if (file) addImage(({backFile:'back',frontFile:'front',logoFile:'logo'})[card.htmlFor] || 'front', file);
      else toast('Drop an image file on an artwork tile.');
    });
  });

  function sgdbStatus(message, kind = '') {
    const status = $('sgdbStatus');
    status.textContent = message;
    status.className = `sgdb-status${kind ? ` ${kind}` : ''}`;
  }

  function sgdbItems(payload) {
    const data = payload && payload.data;
    if (!Array.isArray(data)) return [];
    return data.map((item) => item && item.data && typeof item.data === 'object' ? item.data : item).filter((item) => item && typeof item === 'object');
  }

  function sgdbApiKey() {
    const key = $('sgdbKey').value.trim();
    if (!key) throw new Error('Paste your SteamGridDB API key to search.');
    try {
      if ($('rememberSgdbKey').checked) localStorage.setItem(sgdbKeyStorage, key);
      else localStorage.removeItem(sgdbKeyStorage);
    } catch { /* Storage may be unavailable when this page is opened directly. */ }
    return key;
  }

  async function sgdbRequest(path) {
    const proxyUrl = new URLSearchParams({path});
    const response = await fetch(`/api/steamgriddb?${proxyUrl}`, {
      headers: {Authorization: `Bearer ${sgdbApiKey()}`, Accept: 'application/json'}
    });
    if (!response.ok) {
      let message = '';
      try { message = (await response.json()).error || ''; } catch { /* Keep the status-based fallback. */ }
      if (message) throw new Error(message);
      if (response.status === 401) throw new Error('SteamGridDB rejected this API key. Check it and try again.');
      if (response.status === 429) throw new Error('SteamGridDB asked us to slow down. Wait a little, then retry.');
      throw new Error(`SteamGridDB returned HTTP ${response.status}.`);
    }
    return response.json();
  }

  function showSgdbGames(games) {
    $('sgdbPagination').hidden = true;
    sgdbAssets = [];
    if (!games.length) {
      $('sgdbContent').innerHTML = '';
      sgdbStatus('No matching games found. Try a shorter or alternate title.', 'empty');
      return;
    }
    sgdbStatus('Choose the matching game to browse its artwork.');
    $('sgdbContent').innerHTML = `<div class="sgdb-game-list">${games.map((game) => {
      const gameTypes = Array.isArray(game.types) ? game.types.join(', ') : '';
      return `<button class="sgdb-game-option" type="button" data-game-id="${esc(game.id)}" data-game-name="${esc(game.name || 'Game')}"><span><b>${esc(game.name || 'Unnamed game')}</b><small>${esc(gameTypes || (game.verified ? 'Verified' : 'SteamGridDB game'))}</small></span><span aria-hidden="true">›</span></button>`;
    }).join('')}</div>`;
  }

  async function searchSgdbGames() {
    const term = $('sgdbSearchTerm').value.trim();
    if (!term) { sgdbStatus('Enter a game title to search.', 'error'); return; }
    sgdbGame = null;
    sgdbAssets = [];
    const requestId = ++sgdbRequestId;
    $('sgdbContent').innerHTML = '';
    $('sgdbPagination').hidden = true;
    sgdbStatus('Searching SteamGridDB…');
    try {
      const payload = await sgdbRequest(`search/autocomplete/${encodeURIComponent(term)}`);
      if (requestId !== sgdbRequestId) return;
      showSgdbGames(sgdbItems(payload));
    } catch (error) {
      if (requestId !== sgdbRequestId) return;
      const networkMessage = error instanceof TypeError ? 'Could not reach Plomcase’s API proxy. Start the app with “python server.py” and open it at http://127.0.0.1:8784.' : error.message;
      sgdbStatus(networkMessage, 'error');
    }
  }

  function safeAssetUrl(value) {
    try { const url = new URL(value); return url.protocol === 'https:' ? url.href : ''; }
    catch { return ''; }
  }

  function showSgdbAssets(assets) {
    const content = $('sgdbContent');
    sgdbAssets = assets;
    if (!assets.length) {
      content.innerHTML = '';
      $('sgdbPagination').hidden = true;
      const suffix = sgdbMode === 'grid' && $('sgdbExactSize').checked ? ' Try turning off the 600 × 900 filter.' : '';
      sgdbStatus(`No ${sgdbMode === 'grid' ? 'cover grids' : 'logos'} found for ${sgdbGame.name}.${suffix}`, 'empty');
      return;
    }
    sgdbStatus(`${assets.length} ${sgdbMode === 'grid' ? 'grid' : 'logo'} options for ${sgdbGame.name}. Select one to add it to the case.`);
    content.innerHTML = `<div class="sgdb-assets">${assets.map((asset, index) => {
      const imageUrl = safeAssetUrl(asset.thumb || asset.url);
      const width = Number(asset.width), height = Number(asset.height);
      const detail = [asset.style, width && height ? `${width} × ${height}` : '', asset.author && asset.author.name ? `by ${asset.author.name}` : ''].filter(Boolean).join(' · ');
      const label = sgdbMode === 'grid' ? `Use on ${$('sgdbTarget').value === 'front' ? 'front' : 'back'}` : 'Use on spine';
      return `<article class="sgdb-asset"><div class="sgdb-asset-image">${imageUrl ? `<img src="${esc(imageUrl)}" loading="lazy" alt="${esc(sgdbGame.name)} ${sgdbMode === 'grid' ? 'cover grid' : 'logo'}">` : '<span>Preview unavailable</span>'}</div><div class="sgdb-asset-meta"><small>${esc(detail || 'SteamGridDB artwork')}</small><button class="button" type="button" data-asset-index="${index}">${label}</button></div></article>`;
    }).join('')}</div>`;
    const hasNext = sgdbTotal > (sgdbPage + 1) * 50 || assets.length === 50;
    const pagination = $('sgdbPagination');
    pagination.hidden = sgdbTotal <= 50 && sgdbPage === 0 && assets.length < 50;
    $('sgdbPrev').disabled = sgdbPage === 0;
    $('sgdbNext').disabled = !hasNext;
    $('sgdbPageLabel').textContent = `Page ${sgdbPage + 1}${sgdbTotal ? ` · ${sgdbTotal} total` : ''}`;
  }

  async function loadSgdbAssets(page = 0) {
    if (!sgdbGame) return;
    sgdbPage = page;
    const requestId = ++sgdbRequestId;
    $('sgdbContent').innerHTML = '';
    $('sgdbPagination').hidden = true;
    sgdbStatus(`Loading ${sgdbMode === 'grid' ? 'cover grids' : 'logos'} for ${sgdbGame.name}…`);
    const endpoint = sgdbMode === 'grid' ? `grids/game/${encodeURIComponent(sgdbGame.id)}` : `logos/game/${encodeURIComponent(sgdbGame.id)}`;
    const params = new URLSearchParams({types:'static',nsfw:'false',humor:'false',epilepsy:'false',limit:'50',page:String(page)});
    if (sgdbMode === 'grid' && $('sgdbExactSize').checked) params.set('dimensions', '600x900');
    try {
      const payload = await sgdbRequest(`${endpoint}?${params}`);
      if (requestId !== sgdbRequestId) return;
      sgdbTotal = Number(payload.total) || 0;
      showSgdbAssets(sgdbItems(payload));
    } catch (error) {
      if (requestId !== sgdbRequestId) return;
      const networkMessage = error instanceof TypeError ? 'Could not reach Plomcase’s API proxy. Start the app with “python server.py” and open it at http://127.0.0.1:8784.' : error.message;
      sgdbStatus(networkMessage, 'error');
    }
  }

  function showSgdbPicker() {
    try {
      const storedKey = localStorage.getItem(sgdbKeyStorage) || '';
      $('sgdbKey').value = storedKey;
      $('rememberSgdbKey').checked = Boolean(storedKey);
    } catch { $('sgdbKey').value = ''; $('rememberSgdbKey').checked = false; }
    $('sgdbSearchTerm').value = $('projectName').value.trim();
    sgdbGame = null;
    sgdbPage = 0;
    $('sgdbContent').innerHTML = '';
    $('sgdbPagination').hidden = true;
    sgdbStatus('Enter your API key, then search for a game.');
    $('sgdbDialog').showModal();
  }

  $('openSgdbPicker').addEventListener('click', showSgdbPicker);
  $('closeSgdbPicker').addEventListener('click', () => $('sgdbDialog').close());
  $('sgdbDialog').addEventListener('click', (event) => { if (event.target === $('sgdbDialog')) $('sgdbDialog').close(); });
  $('sgdbSearch').addEventListener('click', searchSgdbGames);
  $('sgdbSearchTerm').addEventListener('keydown', (event) => { if (event.key === 'Enter') searchSgdbGames(); });
  document.querySelectorAll('.sgdb-tab').forEach((tab) => tab.addEventListener('click', () => {
    sgdbMode = tab.dataset.mode;
    document.querySelectorAll('.sgdb-tab').forEach((item) => { const active = item === tab; item.classList.toggle('active', active); item.setAttribute('aria-selected', String(active)); });
    $('sgdbGridOptions').hidden = sgdbMode !== 'grid';
    if (sgdbGame) loadSgdbAssets(0);
    else { $('sgdbContent').innerHTML = ''; sgdbStatus(`Search for a game to browse ${sgdbMode === 'grid' ? 'cover grids' : 'spine logos'}.`); }
  }));
  $('sgdbExactSize').addEventListener('change', () => { if (sgdbGame && sgdbMode === 'grid') loadSgdbAssets(0); });
  $('sgdbTarget').addEventListener('change', () => {
    $('sgdbContent').querySelectorAll('[data-asset-index]').forEach((button) => { button.textContent = `Use on ${$('sgdbTarget').value === 'front' ? 'front' : 'back'}`; });
  });
  $('sgdbContent').addEventListener('click', async (event) => {
    const gameButton = event.target.closest('[data-game-id]');
    if (gameButton) {
      sgdbGame = {id: gameButton.dataset.gameId, name: gameButton.dataset.gameName};
      await loadSgdbAssets(0);
      return;
    }
    const assetButton = event.target.closest('[data-asset-index]');
    if (!assetButton || !sgdbGame) return;
    const asset = sgdbAssets[Number(assetButton.dataset.assetIndex)];
    if (!asset) return;
    const assetUrl = safeAssetUrl(asset.url);
    if (!assetUrl) { sgdbStatus('This artwork has no secure download URL.', 'error'); return; }
    const target = sgdbMode === 'logo' ? 'logo' : $('sgdbTarget').value;
    if (files[target] && !confirm(`Replace the current ${target === 'logo' ? 'spine logo' : `${target} cover`} with this SteamGridDB image?`)) return;
    assetButton.disabled = true;
    sgdbStatus('Downloading artwork into this project…');
    try {
      const imageProxyUrl = new URLSearchParams({url: assetUrl});
      const response = await fetch(`/api/steamgriddb/image?${imageProxyUrl}`);
      if (!response.ok) throw new Error(`Artwork download returned HTTP ${response.status}.`);
      const source = await response.blob();
      const mime = source.type.startsWith('image/') ? source.type : 'image/png';
      let image = await imageData(new Blob([source], {type: mime}));
      if (target === 'logo') image = await normalizeLogo(image);
      files[target] = image;
      const label = `SteamGridDB · ${sgdbGame.name}`;
      $(`${target}FileName`).textContent = label;
      if (target === 'logo') $('removeSpineLogo').hidden = false;
      hasUnsavedChanges = true;
      if (target === 'back' || target === 'front') refreshSpinePalette();
      else update();
      sgdbStatus(`Added ${sgdbMode === 'grid' ? 'cover grid' : 'logo'} for ${sgdbGame.name}.`);
      toast(sgdbMode === 'grid' ? `Grid added to ${target} cover.` : 'Logo added to the spine.');
    } catch (error) {
      const networkMessage = error instanceof TypeError ? 'Could not reach Plomcase’s image proxy. Start the app with “python server.py” and open it at http://127.0.0.1:8784.' : error.message;
      sgdbStatus(networkMessage, 'error');
      assetButton.disabled = false;
    }
  });
  $('sgdbPrev').addEventListener('click', () => { if (sgdbPage > 0) loadSgdbAssets(sgdbPage - 1); });
  $('sgdbNext').addEventListener('click', () => { if (!$('sgdbNext').disabled) loadSgdbAssets(sgdbPage + 1); });
  const markSettingEdited = (id) => {
    if (settingIds.includes(id)) {
      settingOverrides[id] = true;
      renderSettingSources();
    }
    hasUnsavedChanges = true;
    update();
  };
  ids.forEach((id) => $(id).addEventListener('input', () => markSettingEdited(id)));
  ids.forEach((id) => $(id).addEventListener('change', () => markSettingEdited(id)));
  window.addEventListener('storage', (event) => {
    if (event.key !== defaultsStorageKey) return;
    settingDefaults = loadSettingDefaults();
    settingIds.filter((id) => !settingOverrides[id]).forEach((id) => setControlValue(id, settingDefaults[id]));
    renderSettingSources();
    update();
  });
  window.addEventListener('beforeunload', (event) => {
    if (!hasUnsavedChanges) return;
    event.preventDefault();
    event.returnValue = '';
  });

  applySharedDefaultsToControls();
  update();
  listProjects();
})();
