/// The layout audit for the polish pass. Loaded only with
/// `is_auditing`. After fonts load, it measures every screen and writes
/// the findings as JSON into `data-swift-design-findings` on `<html>`, so
/// a DOM dump carries them back to the server. `polish.rs` reads the
/// findings and names the kinds.
pub(crate) const AUDIT_SCRIPT: &str = r##"(async () => {
  if (document.readyState !== 'complete') {
    await new Promise((resolve) => window.addEventListener('load', resolve, { once: true }));
  }
  const main = document.querySelector('main.design');
  const canvasWidth = Number(main && main.dataset.swiftDesignWidth) || 1440;
  const canvasHeight = Number(main && main.dataset.swiftDesignHeight) || 900;
  const isDemo = !!(main && main.classList.contains('design') && main.classList.length === 1);
  const fitThreshold = isDemo ? 0.999 : 0.97;
  const tolerancePx = 2;
  const windowGeometry = main && main.dataset.swiftDesignWindowLeft ? {
    left: Number(main.dataset.swiftDesignWindowLeft),
    top: Number(main.dataset.swiftDesignWindowTop),
    width: Number(main.dataset.swiftDesignWindowWidth),
    height: Number(main.dataset.swiftDesignWindowHeight),
  } : null;
  const hasWindowGeometry = windowGeometry && [windowGeometry.left, windowGeometry.top, windowGeometry.width, windowGeometry.height].every(Number.isFinite);
  // Navigation hides inactive transition frames. Audit every frame, not only the open one.
  document.querySelectorAll('[data-swift-design-frame]').forEach((frame) => {
    frame.style.setProperty('transition', 'none', 'important');
    frame.style.setProperty('animation', 'none', 'important');
    frame.style.setProperty('visibility', 'visible', 'important');
    frame.style.setProperty('opacity', '1', 'important');
    frame.style.setProperty('transform', 'none', 'important');
  });
  if (window.swiftDesignFit) { window.swiftDesignFit(); }
  // Revealing inactive frames can start loading fonts that the first frame did not use.
  if (document.fonts && document.fonts.ready) { try { await document.fonts.ready; } catch (error) {} }
  if (window.swiftDesignFit) { window.swiftDesignFit(); }
  const findings = [];
  const auditedScreens = [];
  const hasOwnText = (element) => Array.from(element.childNodes).some((node) => node.nodeType === 3 && node.textContent.trim());
  const isClosedDetailsDescendant = (element) => {
    const details = element.closest('details:not([open])');
    return !!(details && element !== details && element.tagName.toLowerCase() !== 'summary' && !element.closest('summary'));
  };
  const isVisible = (element) => {
    if (isClosedDetailsDescendant(element)) { return false; }
    for (let node = element; node && node.nodeType === 1; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse' || Number(style.opacity) === 0) { return false; }
      if (node.hasAttribute('hidden')) { return false; }
    }
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const overflowMode = (element) => {
    const style = getComputedStyle(element);
    return { x: style.overflowX, y: style.overflowY };
  };
  const isIntentionalScrollport = (element) => {
    const mode = overflowMode(element);
    return /^(auto|scroll)$/.test(mode.x) || /^(auto|scroll)$/.test(mode.y);
  };
  const isInsideScrollport = (element, root) => {
    let node = element.parentElement;
    while (node && node !== root) {
      if (isIntentionalScrollport(node)) { return true; }
      node = node.parentElement;
    }
    return false;
  };
  const rectIntersects = (a, b, tolerance) =>
    a.right > b.left + tolerance && a.left < b.right - tolerance && a.bottom > b.top + tolerance && a.top < b.bottom - tolerance;
  const visibleChildrenOf = (element, rootRect) => Array.from(element.children).filter((child) => {
    if (!isVisible(child)) { return false; }
    const rect = child.getBoundingClientRect();
    return rectIntersects(rect, rootRect, 0);
  });
  const axisValue = (rect, mode) => {
    if (mode === 'left') { return rect.left; }
    if (mode === 'right') { return rect.right; }
    if (mode === 'top') { return rect.top; }
    if (mode === 'bottom') { return rect.bottom; }
    if (mode === 'center-x') { return (rect.left + rect.right) / 2; }
    if (mode === 'center-y') { return (rect.top + rect.bottom) / 2; }
    if (mode === 'width') { return rect.width; }
    if (mode === 'height') { return rect.height; }
    return null;
  };
  const alignmentModes = new Set(['left', 'right', 'top', 'bottom', 'center-x', 'center-y', 'width', 'height']);
  const gapModes = new Set(['x', 'y']);
  document.querySelectorAll('[data-swift-design-root]').forEach((root) => {
    const screen = Number(root.closest('[data-swift-design-screen]').dataset.swiftDesignScreen);
    auditedScreens.push(screen);
    const rootRect = root.getBoundingClientRect();
    const scale = rootRect.width / canvasWidth || 1;
    const tolerance = tolerancePx * scale;
    const pathOf = (element) => {
      const parts = [];
      let node = element;
      while (node && node !== root) { const parent = node.parentElement; if (!parent) { break; } parts.unshift(Array.prototype.indexOf.call(parent.children, node)); node = parent; }
      return parts.join('/');
    };
    const name = (element) => element.tagName.toLowerCase() + (element.classList[0] ? '.' + element.classList[0] : '') + ' (' + pathOf(element) + ')';
    const visibleAll = Array.from(root.querySelectorAll('*')).filter(isVisible);
    if (!hasOwnText(root) && !visibleAll.some((element) => hasOwnText(element) || element.matches('img, svg'))) {
      findings.push({ screen, node: 'root', kind: 'empty', detail: 'the screen has no visible text, image, or svg' });
    }
    // Scroll extents include trailing padding. Check painted content before declaring clipping.
    const hasVisibleOverflow = (element, axis) => {
      const rectangle = element.getBoundingClientRect();
      const axisScale = axis === 'x' ? rectangle.width / element.offsetWidth : rectangle.height / element.offsetHeight;
      const factor = axisScale || scale;
      const start = axis === 'x' ? rectangle.left + element.clientLeft * factor : rectangle.top + element.clientTop * factor;
      const end = start + (axis === 'x' ? element.clientWidth : element.clientHeight) * factor;
      const outside = (rectangle) => axis === 'x'
        ? rectangle.left < start - tolerance || rectangle.right > end + tolerance
        : rectangle.top < start - tolerance || rectangle.bottom > end + tolerance;
      let hasMeasuredContent = false;
      const nodes = [element, ...Array.from(element.querySelectorAll('*')).filter((node) => isVisible(node) && !isInsideScrollport(node, element))];
      for (const node of nodes) {
        if (node !== element) {
          hasMeasuredContent = true;
          if (outside(node.getBoundingClientRect())) { return true; }
        }
        // Generated content has no text node to measure. Keep its scroll warning conservatively.
        if (['::before', '::after'].some((pseudo) => {
          const content = getComputedStyle(node, pseudo).content;
          return content && content !== 'none' && content !== 'normal' && content !== '""';
        })) { return true; }
        for (const child of node.childNodes) {
          if (child.nodeType !== Node.TEXT_NODE || !child.textContent.trim()) { continue; }
          const range = document.createRange();
          range.selectNodeContents(child);
          for (const rectangle of range.getClientRects()) {
            if (!rectangle.width || !rectangle.height) { continue; }
            hasMeasuredContent = true;
            if (outside(rectangle)) { return true; }
          }
        }
      }
      return !hasMeasuredContent;
    };
    const overflowAxes = new Map();
    const overflowing = visibleAll.filter((element) => {
      if (element.closest('svg') || isIntentionalScrollport(element)) { return false; }
      const x = element.clientWidth > 0 && element.scrollWidth > element.clientWidth + 2 && hasVisibleOverflow(element, 'x');
      const y = element.clientHeight > 0 && element.scrollHeight > element.clientHeight + 2 && hasVisibleOverflow(element, 'y');
      overflowAxes.set(element, { x, y });
      return x || y;
    });
    overflowing.forEach((element) => {
      if (overflowing.some((other) => other !== element && element.contains(other))) { return; }
      const needs = [];
      if (overflowAxes.get(element).x) { needs.push(Math.round(element.scrollWidth) + 'px wide but the box is ' + Math.round(element.clientWidth) + 'px wide'); }
      if (overflowAxes.get(element).y) { needs.push(Math.round(element.scrollHeight) + 'px tall but the box is ' + Math.round(element.clientHeight) + 'px tall'); }
      findings.push({ screen, node: name(element), kind: 'overflow', detail: 'content needs ' + needs.join(' and ') });
    });
    if (root.scrollHeight > root.clientHeight + 2 || root.scrollWidth > root.clientWidth + 2) {
      findings.push({ screen, node: 'root', kind: 'off_screen', detail: 'content runs past the ' + canvasWidth + ' by ' + canvasHeight + ' canvas (' + Math.round(root.scrollWidth) + ' by ' + Math.round(root.scrollHeight) + 'px)' });
    }
    const fitFactor = Number(root.dataset.swiftDesignFit || 1);
    if (hasWindowGeometry) {
      const windowElement = Array.from(root.children).find((child) => child.hasAttribute('data-swift-design-window'));
      const windowTolerance = 1.5;
      const differs = (actual, expected) => Math.abs(actual - expected) > windowTolerance;
      if (!windowElement) {
        findings.push({ screen, node: 'root', kind: 'window_geometry', detail: 'missing direct child marked data-swift-design-window for the app window geometry' });
      } else {
        const rect = windowElement.getBoundingClientRect();
        const actual = {
          left: (rect.left - rootRect.left) / scale,
          top: (rect.top - rootRect.top) / scale,
          width: rect.width / scale,
          height: rect.height / scale,
        };
        if (differs(actual.left, windowGeometry.left) || differs(actual.top, windowGeometry.top) || differs(actual.width, windowGeometry.width) || differs(actual.height, windowGeometry.height)) {
          findings.push({ screen, node: 'data-swift-design-window', kind: 'window_geometry', detail: 'app window is at ' + Math.round(actual.left) + ',' + Math.round(actual.top) + ' ' + Math.round(actual.width) + ' by ' + Math.round(actual.height) + 'px but metadata requires ' + windowGeometry.left + ',' + windowGeometry.top + ' ' + windowGeometry.width + ' by ' + windowGeometry.height + 'px' });
        }
      }
      if (fitFactor < 0.999) {
        findings.push({ screen, node: 'root', kind: 'window_geometry', detail: 'the app window geometry forced the screen to scale to ' + Math.round(fitFactor * 1000) / 10 + '%: keep the marked window inside the canvas' });
      }
    }
    if (fitFactor < fitThreshold) {
      findings.push({ screen, node: 'root', kind: 'overfull', detail: 'the screen holds more than the ' + canvasWidth + ' by ' + canvasHeight + ' canvas fits, so it was scaled to ' + Math.round(fitFactor * 100) + '%: cut content or reduce the sizes' });
    }
    const controlPattern = /(^|[-_ ])(button|btn|cta|toggle|switch|tab|menu|dropdown)([-_ ]|$)/i;
    const acts = (element) => {
      const tag = element.tagName.toLowerCase();
      const href = element.getAttribute('href') || '';
      if (tag === 'a') { return /^#screen-\d+$/.test(href) || /^(https?:|mailto:|tel:)/i.test(href); }
      if (tag === 'label') { return !!element.getAttribute('for'); }
      return tag === 'summary' || tag === 'details' || tag === 'input';
    };
    visibleAll.forEach((element) => {
      if (element.closest('svg')) { return; }
      const tag = element.tagName.toLowerCase();
      const looksLikeControl = tag === 'a' || tag === 'button' || element.getAttribute('role') === 'button' || controlPattern.test(element.getAttribute('class') || '');
      if (!looksLikeControl || acts(element)) { return; }
      if (element.closest('a[href], label[for], summary') || element.querySelector('a[href], label[for], summary, input')) { return; }
      findings.push({ screen, node: name(element), kind: 'static_control', detail: 'looks like a control but does nothing when clicked' });
    });
    visibleAll.forEach((element) => {
      const align = element.getAttribute('data-layout-align');
      if (align !== null) {
        if (!alignmentModes.has(align)) {
          findings.push({ screen, node: name(element), kind: 'alignment', detail: 'data-layout-align must be one of left, right, top, bottom, center-x, center-y, width, or height' });
        } else {
          const children = visibleChildrenOf(element, rootRect);
          if (children.length < 2) {
            findings.push({ screen, node: name(element), kind: 'alignment', detail: 'data-layout-align="' + align + '" needs at least 2 visible direct children' });
          } else {
            const values = children.map((child) => ({ child, value: axisValue(child.getBoundingClientRect(), align) / scale }));
            const expected = values[0].value;
            const offender = values.find((entry) => Math.abs(entry.value - expected) > tolerancePx);
            if (offender) {
              findings.push({ screen, node: name(element), kind: 'alignment', detail: 'direct children marked for ' + align + ' alignment differ by more than ' + tolerancePx + ' canvas px' });
            }
          }
        }
      }
      const gap = element.getAttribute('data-layout-gap');
      if (gap !== null) {
        if (!gapModes.has(gap)) {
          findings.push({ screen, node: name(element), kind: 'alignment', detail: 'data-layout-gap must be x or y' });
        } else {
          const children = visibleChildrenOf(element, rootRect).sort((a, b) => {
            const ar = a.getBoundingClientRect(), br = b.getBoundingClientRect();
            return gap === 'x' ? ar.left - br.left : ar.top - br.top;
          });
          if (children.length < 3) {
            findings.push({ screen, node: name(element), kind: 'alignment', detail: 'data-layout-gap="' + gap + '" needs at least 3 visible direct children' });
          } else {
            const gaps = [];
            for (let index = 1; index < children.length; index += 1) {
              const previous = children[index - 1].getBoundingClientRect();
              const current = children[index].getBoundingClientRect();
              gaps.push((gap === 'x' ? current.left - previous.right : current.top - previous.bottom) / scale);
            }
            const expected = gaps[0];
            if (gaps.some((value) => Math.abs(value - expected) > tolerancePx)) {
              findings.push({ screen, node: name(element), kind: 'alignment', detail: 'adjacent ' + (gap === 'x' ? 'horizontal' : 'vertical') + ' gaps differ by more than ' + tolerancePx + ' canvas px' });
            }
          }
        }
      }
    });
    const textFloor = canvasWidth >= 1920 ? { flag: 20, ask: 24 } : canvasWidth >= 1000 ? { flag: 12, ask: 14 } : { flag: 11, ask: 12 };
    const textBlocks = [];
    visibleAll.forEach((element) => {
      if (element.closest('svg') && element.tagName.toLowerCase() !== 'svg') { return; }
      const isText = hasOwnText(element);
      const tag = element.tagName.toLowerCase();
      if (!isText && tag !== 'img' && tag !== 'svg') { return; }
      const rect = element.getBoundingClientRect();
      if (!isInsideScrollport(element, root) && (rect.left < rootRect.left - tolerance || rect.top < rootRect.top - tolerance || rect.right > rootRect.right + tolerance || rect.bottom > rootRect.bottom + tolerance)) {
        findings.push({ screen, node: name(element), kind: 'off_screen', detail: 'runs off the screen: keep it inside the ' + canvasWidth + ' by ' + canvasHeight + ' canvas' });
      }
      if (isText) {
        const size = parseFloat(getComputedStyle(element).fontSize);
        if (size < textFloor.flag) {
          findings.push({ screen, node: name(element), kind: 'tiny_text', detail: 'font-size ' + Math.round(size) + 'px is too small: use at least ' + textFloor.ask + 'px' });
        }
        if (!isInsideScrollport(element, root)) { textBlocks.push({ element, rect }); }
      }
    });
    const parseColor = (value) => {
      const match = /rgba?\(([^)]+)\)/.exec(value || '');
      if (!match) { return null; }
      const parts = match[1].split(',').map((part) => parseFloat(part));
      if (parts.length >= 4 && parts[3] === 0) { return null; }
      return parts.slice(0, 3);
    };
    const rgbText = (rgb) => 'rgb(' + rgb.map((part) => Math.round(part)).join(', ') + ')';
    const luminance = (rgb) => {
      const channel = (value) => { const c = value / 255; return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4); };
      return 0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2]);
    };
    const contrastRatio = (a, b) => { const la = luminance(a), lb = luminance(b); return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05); };
    const section = root.closest('[data-swift-design-screen]');
    const backgroundOf = (element) => {
      let node = element;
      while (node && node !== section.parentElement) {
        const color = parseColor(getComputedStyle(node).backgroundColor);
        if (color) { return color; }
        node = node.parentElement;
      }
      return null;
    };
    textBlocks.forEach(({ element, rect }) => {
      const style = getComputedStyle(element);
      const size = parseFloat(style.fontSize);
      const weight = parseInt(style.fontWeight, 10) || 400;
      const limit = size >= 24 || (weight >= 700 && size >= 18.66) ? 3.0 : 4.5;
      const text = parseColor(style.color);
      const background = backgroundOf(element);
      if (text && background) {
        const ratio = contrastRatio(text, background);
        if (ratio < limit) {
          findings.push({ screen, node: name(element), kind: 'contrast', detail: 'contrast ' + ratio.toFixed(2) + ':1 is below the ' + limit.toFixed(1) + ':1 limit for ' + rgbText(text) + ' text on ' + rgbText(background) });
        }
      }
      const lineHeight = parseFloat(style.lineHeight) || size * 1.2;
      const lines = Math.max(1, Math.round(rect.height / scale / lineHeight));
      const perLine = element.textContent.trim().length / lines;
      if (perLine > 100) {
        findings.push({ screen, node: name(element), kind: 'long_lines', detail: 'about ' + Math.round(perLine) + ' characters per line over ' + lines + (lines === 1 ? ' line' : ' lines') + ': keep lines under 100 characters' });
      }
    });
    let overlaps = 0;
    for (let first = 0; first < textBlocks.length && overlaps < 6; first += 1) {
      for (let second = first + 1; second < textBlocks.length && overlaps < 6; second += 1) {
        const a = textBlocks[first], b = textBlocks[second];
        if (a.element.contains(b.element) || b.element.contains(a.element)) { continue; }
        if (isInsideScrollport(a.element, root) || isInsideScrollport(b.element, root)) { continue; }
        const width = Math.min(a.rect.right, b.rect.right) - Math.max(a.rect.left, b.rect.left);
        const height = Math.min(a.rect.bottom, b.rect.bottom) - Math.max(a.rect.top, b.rect.top);
        if (width > 4 * scale && height > 4 * scale) {
          overlaps += 1;
          findings.push({ screen, node: name(a.element) + ' and ' + name(b.element), kind: 'overlap', detail: 'the text blocks overlap: move or resize one' });
        }
      }
    }
  });
  document.documentElement.setAttribute('data-swift-design-findings', JSON.stringify(findings));
  document.documentElement.setAttribute('data-swift-design-audited-screens', JSON.stringify(auditedScreens));
})();
"##;
