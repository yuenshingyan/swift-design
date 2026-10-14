/// Fits each screen's content inside the canvas.
///
/// The root is a fixed canvas box with `overflow: hidden`, so content
/// that needs more room is cut off. Nothing in the pipeline guarantees
/// the model writes content that fits. This grows the box until the
/// content fits and scales the whole root back to the canvas, so the
/// slide is smaller but whole.
///
/// The factor lands in `--swift-design-fit`, which the stylesheet
/// multiplies into the root transform, and in `data-swift-design-fit`,
/// which the audit reads and reports.
pub(crate) const FIT_SCRIPT: &str = r##"(() => {
  const main = document.querySelector('main.design');
  const canvasWidth = Number(main && main.dataset.swiftDesignWidth) || 1440;
  const canvasHeight = Number(main && main.dataset.swiftDesignHeight) || 900;
  // Below this a screen is unreadable, so it stays cut off and the audit
  // reports it instead.
  const FIT_FLOOR = 0.4;
  const PASSES = 6;
  // The box keeps the canvas ratio at every factor, so a screen built
  // around full-height boxes and centred flex rows still measures the
  // way it will be shown.
  // An agent often wraps a screen in a box of its own that has the
  // canvas size and clips. That box hides its overflow from the root,
  // so the fit marks it: the stylesheet sizes it with the root, and the
  // text inside it is measured against the root box.
  const markInnerRoot = (root) => {
    const children = Array.from(root.children);
    const child = children.length === 1 ? children[0] : null;
    if (child && child.offsetWidth >= canvasWidth - 2 && child.offsetHeight >= canvasHeight - 2) {
      child.dataset.swiftDesignInnerRoot = '';
    }
  };
  // Only an element that holds text or media counts, its box included:
  // a decoration that bleeds past the edge is meant to be clipped.
  const hasContent = (element) =>
    element.matches('img, svg, video, canvas, input, textarea') ||
    element.textContent.trim() !== '';
  const isClippedOrHidden = (element, root) => {
    const details = element.closest('details:not([open])');
    if (details && element !== details && element.tagName.toLowerCase() !== 'summary' && !element.closest('summary')) { return true; }
    for (let node = element; node && node !== root; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse' || Number(style.opacity) === 0) { return true; }
      if (node !== element && (/^(auto|scroll)$/.test(style.overflowX) || /^(auto|scroll)$/.test(style.overflowY))) { return true; }
    }
    return false;
  };
  const contentOverflows = (root) => {
    const box = root.getBoundingClientRect();
    const scale = box.width / (root.clientWidth || 1);
    const bottom = box.bottom + 2 * scale;
    const right = box.right + 2 * scale;
    return Array.from(root.querySelectorAll('*')).some((element) => {
      if (element.closest('svg') && element.tagName.toLowerCase() !== 'svg') { return false; }
      if (element.hasAttribute('data-swift-design-inner-root') || !hasContent(element) || isClippedOrHidden(element, root)) { return false; }
      const rect = element.getBoundingClientRect();
      return rect.bottom > bottom || rect.right > right;
    });
  };
  const overflowsAt = (root, factor) => {
    root.style.width = Math.round(canvasWidth / factor) + 'px';
    root.style.height = Math.round(canvasHeight / factor) + 'px';
    if (root.scrollHeight > root.clientHeight + 2 || root.scrollWidth > root.clientWidth + 2) { return true; }
    return root.querySelector('[data-swift-design-inner-root]') !== null && contentOverflows(root);
  };
  const fitRoot = (root) => {
    let factor = 1;
    if (overflowsAt(root, 1)) {
      // The largest factor that still fits, to 1/64th. A smaller factor
      // always fits when a larger one does, so this is a plain search.
      let low = FIT_FLOOR;
      let high = 1;
      for (let pass = 0; pass < PASSES; pass += 1) {
        const middle = (low + high) / 2;
        if (overflowsAt(root, middle)) { high = middle; } else { low = middle; }
      }
      factor = low;
      overflowsAt(root, factor);
    }
    root.style.setProperty('--swift-design-fit', String(factor));
    root.dataset.swiftDesignFit = factor.toFixed(4);
  };
  const fitAll = () => {
    if (window.swiftDesignApplyWindow) { window.swiftDesignApplyWindow(); }
    document.querySelectorAll('[data-swift-design-root]').forEach((root) => { markInnerRoot(root); fitRoot(root); });
  };
  fitAll();
  // A web font changes the metrics, so the fit is measured again once
  // the fonts are in.
  if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(fitAll).catch(() => {});
  }
  window.swiftDesignFit = fitAll;
})();
"##;

/// Places the app window shell at the shared geometry for each screen.
///
/// Design models can name one app window rectangle for all screens. The
/// model writes a direct child marked `data-swift-design-window` under
/// each root. This script owns the hard geometry and writes it inline with
/// `!important`, before the fit script measures the content.
pub(crate) const DESIGN_WINDOW_SCRIPT: &str = r##"(() => {
  const main = document.querySelector('main.design');
  if (!main || !main.dataset.swiftDesignWindowLeft) { return; }
  const geometry = {
    left: Number(main.dataset.swiftDesignWindowLeft),
    top: Number(main.dataset.swiftDesignWindowTop),
    width: Number(main.dataset.swiftDesignWindowWidth),
    height: Number(main.dataset.swiftDesignWindowHeight),
  };
  if (![geometry.left, geometry.top, geometry.width, geometry.height].every(Number.isFinite)) { return; }
  const setImportant = (style, property, value) => style.setProperty(property, value, 'important');
  const apply = () => document.querySelectorAll('[data-swift-design-root]').forEach((root) => {
    const windowElement = Array.from(root.children).find((child) => child.hasAttribute('data-swift-design-window'));
    if (!windowElement) { return; }
    const style = windowElement.style;
    const properties = ['position', 'left', 'top', 'right', 'bottom', 'width', 'height', 'min-width', 'max-width', 'min-height', 'max-height', 'margin-top', 'margin-right', 'margin-bottom', 'margin-left', 'box-sizing', 'transform', 'translate', 'rotate', 'scale'];
    if (!windowElement.hasAttribute('data-swift-design-window-applied')) {
      const original = properties.map((property) => [property, style.getPropertyValue(property), style.getPropertyPriority(property)]);
      windowElement.dataset.swiftDesignWindowApplied = JSON.stringify(original);
    }
    setImportant(style, 'position', 'absolute');
    setImportant(style, 'left', geometry.left + 'px');
    setImportant(style, 'top', geometry.top + 'px');
    setImportant(style, 'right', 'auto');
    setImportant(style, 'bottom', 'auto');
    setImportant(style, 'width', geometry.width + 'px');
    setImportant(style, 'height', geometry.height + 'px');
    setImportant(style, 'min-width', geometry.width + 'px');
    setImportant(style, 'max-width', geometry.width + 'px');
    setImportant(style, 'min-height', geometry.height + 'px');
    setImportant(style, 'max-height', geometry.height + 'px');
    setImportant(style, 'margin', '0');
    setImportant(style, 'box-sizing', 'border-box');
    setImportant(style, 'transform', 'none');
    setImportant(style, 'translate', 'none');
    setImportant(style, 'rotate', 'none');
    setImportant(style, 'scale', 'none');
  });
  window.swiftDesignApplyWindow = apply;
  apply();
})();
"##;

/// Fits each viewport-sized root to its frame by setting the scale
/// variable from the measured frame width. The stylesheet carries a
/// CSS-only fallback, but container units inside `atan2()` do not
/// resolve in every browser, so the script is the primary path. The
/// canvas width comes from `data-swift-design-width` on `main.design`.
pub(crate) const LAYOUT_SCRIPT: &str = r##"(() => {
  const main = document.querySelector('main.design');
  const canvasWidth = Number(main && main.dataset.swiftDesignWidth) || 1440;
  const sections = Array.from(document.querySelectorAll('[data-swift-design-screen]'));
  const fit = (section) => {
    const width = section.getBoundingClientRect().width;
    if (width > 0) { section.style.setProperty('--swift-design-scale', String(width / canvasWidth)); }
  };
  sections.forEach(fit);
  if (window.ResizeObserver) {
    const observer = new ResizeObserver((entries) => entries.forEach((entry) => fit(entry.target)));
    sections.forEach((section) => observer.observe(section));
  }
  window.addEventListener('resize', () => sections.forEach(fit));
})();
"##;

/// Moves between screens with ArrowLeft/ArrowRight and PageUp/PageDown,
/// and opens screen N on a click on a link to `#screen-N` (counted from
/// 1). A design with no transition scrolls. A design with one stacks its frames
/// and swaps the `data-swift-design-state` attribute, which the transition
/// CSS animates. On a single-screen page inside the editor, asks the
/// editor instead. A page that follows a presenter (`isFollowing`)
/// ignores its own keys and wheel; the deck audience script moves it.
pub(crate) const NAVIGATION_SCRIPT: &str = r##"const frames = Array.from(document.querySelectorAll('[data-swift-design-frame]'));
const design = document.querySelector('main.design');
const effect = design && design.getAttribute('data-swift-design-effect');
const canvasWidth = Number(design && design.dataset.swiftDesignWidth) || 1440;
const canvasHeight = Number(design && design.dataset.swiftDesignHeight) || 900;
const isFollowing = !!(design && design.dataset.swiftDesignChannel);
function scrollIndex() {
  return Math.round(design.scrollTop / (design.clientHeight || 1));
}
let shown = 0;
let isBusy = false;
let queued = null;
function stack(next, sign) {
  if (next === shown || next < 0 || next >= frames.length) { return; }
  if (isBusy) { queued = { next, sign }; return; }
  const duration = Number(design.dataset.swiftDesignDuration || 0);
  design.style.setProperty('--swift-design-sign', String(sign));
  const leaving = frames[shown];
  const entering = frames[next];
  entering.setAttribute('data-swift-design-state', 'entering');
  void entering.offsetWidth;
  entering.setAttribute('data-swift-design-state', 'current');
  leaving.setAttribute('data-swift-design-state', 'leaving');
  shown = next;
  isBusy = true;
  setTimeout(() => {
    if (leaving.getAttribute('data-swift-design-state') === 'leaving') { leaving.removeAttribute('data-swift-design-state'); }
    isBusy = false;
    if (queued) { const pending = queued; queued = null; stack(pending.next, pending.sign); }
  }, duration);
}
function show(target, sign, isInstant) {
  if (target < 0 || target >= frames.length) { return; }
  if (effect) { stack(target, sign); return; }
  frames[target].scrollIntoView({ behavior: isInstant ? 'instant' : 'smooth' });
}
function step(amount) {
  if (frames.length <= 1 && window.parent !== window) {
    parent.postMessage({ type: 'swift-design-navigate', step: amount }, window.location.origin);
    return;
  }
  const current = effect ? shown : scrollIndex();
  const target = Math.min(Math.max(current + amount, 0), frames.length - 1);
  show(target, amount >= 0 ? 1 : -1);
}
if (effect && frames.length) { frames[0].setAttribute('data-swift-design-state', 'current'); }
document.addEventListener('keydown', (event) => {
  if (isFollowing || (event.target && event.target.isContentEditable)) { return; }
  const isNext = event.key === 'ArrowRight' || event.key === 'PageDown';
  const isPrevious = event.key === 'ArrowLeft' || event.key === 'PageUp';
  if (!isNext && !isPrevious) { return; }
  step(isNext ? 1 : -1);
});
if (effect && !isFollowing) {
  let wheelAt = 0;
  window.addEventListener('wheel', (event) => {
    const amount = Math.abs(event.deltaY) >= Math.abs(event.deltaX) ? event.deltaY : event.deltaX;
    if (Math.abs(amount) < 12 || event.timeStamp - wheelAt < 320) { return; }
    wheelAt = event.timeStamp;
    step(amount > 0 ? 1 : -1);
  }, { passive: true });
}
// A link to `#screen-N` opens screen N, counted from 1, as the button of
// a flow. A single-screen page inside the editor asks the editor to open
// it. The editing script stops the click before it gets here, so a link
// only selects in edit mode.
document.addEventListener('click', (event) => {
  const link = event.target && event.target.closest ? event.target.closest('a[href]') : null;
  if (!link) { return; }
  const match = /^#screen-(\d+)$/.exec(link.getAttribute('href') || '');
  if (!match) { return; }
  event.preventDefault();
  const target = Number(match[1]) - 1;
  if (frames.length <= 1 && window.parent !== window) {
    parent.postMessage({ type: 'swift-design-navigate', target }, window.location.origin);
    return;
  }
  const current = effect ? shown : scrollIndex();
  show(target, target >= current ? 1 : -1);
});
"##;

mod editing;
pub(crate) use editing::EDITING_SCRIPT;

mod audit;
pub(crate) use audit::AUDIT_SCRIPT;
