//! Renders a validated design to one standalone HTML page.
//!
//! Every screen is a px canvas the size of the design's viewport, scaled
//! with CSS to fit its frame, so the same layout appears in thumbnails,
//! the editor, and screenshots. Screen `html` is inserted as written and
//! screen `css` is scoped to the screen. Validation (`design_model::markup`)
//! rejected unsafe markup before this point; a hash-based CSP is the
//! second layer. Callers must run `Design::validate` first.

use design_model::transition::MAX_TRANSITION_MS;
use design_model::{Design, Screen, Theme, Transition, TransitionEffect, Viewport};
use sha2::{Digest, Sha256};

use crate::export::base64_encode;
use crate::screen_css::{google_fonts_link, scope_css};

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
  const contentOverflows = (root) => {
    const box = root.getBoundingClientRect();
    const scale = box.width / (root.clientWidth || 1);
    const bottom = box.bottom + 2 * scale;
    const right = box.right + 2 * scale;
    return Array.from(root.querySelectorAll('*')).some((element) => {
      if (element.closest('svg') && element.tagName.toLowerCase() !== 'svg') { return false; }
      if (element.hasAttribute('data-swift-design-inner-root') || !hasContent(element)) { return false; }
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
  const fitAll = () => document.querySelectorAll('[data-swift-design-root]').forEach((root) => { markInnerRoot(root); fitRoot(root); });
  fitAll();
  // A web font changes the metrics, so the fit is measured again once
  // the fonts are in.
  if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(fitAll).catch(() => {});
  }
  window.swiftDesignFit = fitAll;
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

/// In-place editing for the editor preview. Loaded only with
/// `editable=true`, so plain render and export output stays inert.
/// Clicking a node selects it and posts its path; dragging a node moves
/// it; double-clicking a text node edits it in place; a right-click menu
/// applies quick actions in the DOM; every change posts the screen root's
/// HTML back to the editor. Same-origin messages only.
pub(crate) const EDITING_SCRIPT: &str = r##"const origin = window.location.origin;
const editingStyle = document.createElement('style');
editingStyle.textContent = `
[data-swift-design-root] *:hover:not(:has(:hover)) { outline: 1px dashed rgba(14, 110, 99, 0.6) !important; outline-offset: -1px; cursor: pointer; }
[data-swift-design-selected] { outline: 2px solid #0E6E63 !important; outline-offset: -2px; }
[contenteditable]:focus { outline: 2px solid #0E6E63 !important; }
[data-swift-design-dragging] { cursor: grabbing !important; }
[data-swift-design-dragging] * { cursor: grabbing !important; user-select: none !important; }
.swift-design-menu { position: fixed; z-index: 10; display: none; min-width: 12rem;
  background: #FFFFFF; color: #15181C; border: 1px solid #DAD7D0; border-radius: 8px;
  box-shadow: 0 14px 34px -18px rgba(21, 24, 28, 0.5); padding: 0.3rem;
  font: 500 13px Inter, system-ui, sans-serif; }
.swift-design-menu button { display: block; width: 100%; text-align: left; border: 0;
  background: transparent; color: inherit; font: inherit; padding: 0.4rem 0.7rem;
  border-radius: 5px; cursor: pointer; }
.swift-design-menu button:hover { background: #F1EFEA; }
.swift-design-menu hr { border: 0; border-top: 1px solid #EAE7E1; margin: 0.25rem 0; }
.swift-design-menu label { display: flex; align-items: center; gap: 0.6rem;
  padding: 0.3rem 0.7rem; cursor: pointer; }
.swift-design-menu input[type='color'] { width: 1.8rem; height: 1.4rem; border: 1px solid #DAD7D0;
  border-radius: 4px; padding: 0; background: #FFFFFF; }
`;
document.head.appendChild(editingStyle);
const menu = document.createElement('div');
menu.className = 'swift-design-menu';
document.body.appendChild(menu);
function hideMenu() { menu.style.display = 'none'; menu.innerHTML = ''; }
document.addEventListener('click', hideMenu);
document.addEventListener('keydown', (event) => { if (event.key === 'Escape') { hideMenu(); if (document.activeElement) { document.activeElement.blur(); } } });

function post(message) { parent.postMessage(message, origin); }
function rootOf(node) { return node.closest('[data-swift-design-root]'); }
function screenIndexOf(root) { return Number(root.closest('[data-swift-design-screen]').dataset.swiftDesignScreen); }
function pathOf(element) {
  const root = rootOf(element);
  const parts = [];
  let node = element;
  while (node && node !== root) {
    const parent = node.parentElement;
    if (!parent) { break; }
    parts.unshift(Array.prototype.indexOf.call(parent.children, node));
    node = parent;
  }
  return parts.join('/');
}
function nodeAt(root, path) {
  if (path === null || path === undefined || path === '') { return root; }
  let node = root;
  for (const part of String(path).split('/')) {
    node = node.children[Number(part)];
    if (!node) { return null; }
  }
  return node;
}
function serialize(root) {
  const clone = root.cloneNode(true);
  // The editing and fit scripts mark nodes with reserved attributes.
  // None of them belong in the saved HTML.
  clone.querySelectorAll('[contenteditable], [spellcheck], [data-swift-design-selected], [data-swift-design-inner-root], [data-swift-design-dragging]').forEach((node) => {
    node.removeAttribute('contenteditable');
    node.removeAttribute('spellcheck');
    node.removeAttribute('data-swift-design-selected');
    node.removeAttribute('data-swift-design-inner-root');
    node.removeAttribute('data-swift-design-dragging');
  });
  return clone.innerHTML;
}
function postHtml(root, save) {
  // An edit changes how much the screen holds, so the fit is measured
  // again before the change leaves the page.
  if (window.swiftDesignFit) { window.swiftDesignFit(); }
  post({ type: 'swift-design-html', screen: screenIndexOf(root), html: serialize(root), save: !!save });
}
function toHex(color) {
  const match = /rgba?\((\d+),\s*(\d+),\s*(\d+)(?:,\s*([\d.]+))?/.exec(color || '');
  if (!match || (match[4] !== undefined && Number(match[4]) === 0)) { return null; }
  return '#' + [match[1], match[2], match[3]].map((part) => Number(part).toString(16).padStart(2, '0')).join('');
}
function hasOwnText(element) {
  return Array.from(element.childNodes).some((node) => node.nodeType === 3 && node.textContent.trim());
}
function describe(element) {
  const style = getComputedStyle(element);
  return {
    path: pathOf(element),
    tag: element.tagName.toLowerCase(),
    classes: element.getAttribute('class') || '',
    text: (element.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 80),
    styles: {
      font_size: style.fontSize,
      color: toHex(style.color) || '',
      text_align: style.textAlign,
      padding: style.padding,
      src: element.getAttribute('src') || '',
      is_leaf: hasOwnText(element) && element.children.length === 0,
    },
  };
}
let selected = null;
// A plain click selects one node. A click with the command key (or
// control) adds the node to the selection, or removes it again. The
// last node clicked is the primary one: the inspector shows it, and
// the chat names every node in the selection.
let selection = [];
function brief(element) {
  return {
    path: pathOf(element),
    tag: element.tagName.toLowerCase(),
    classes: element.getAttribute('class') || '',
    text: (element.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 80),
  };
}
function select(element, isAdditive) {
  if (!element) { selection = []; }
  else if (isAdditive) {
    const index = selection.indexOf(element);
    if (index >= 0) { selection.splice(index, 1); } else { selection.push(element); }
  } else { selection = [element]; }
  document.querySelectorAll('[data-swift-design-selected]').forEach((node) => node.removeAttribute('data-swift-design-selected'));
  selection.forEach((node) => node.setAttribute('data-swift-design-selected', ''));
  selected = selection.length ? selection[selection.length - 1] : null;
  if (!element) { return; }
  const root = rootOf(element);
  if (!selected) { post({ type: 'swift-design-select', screen: screenIndexOf(root), path: null }); return; }
  post(Object.assign({ type: 'swift-design-select', screen: screenIndexOf(root), selection: selection.map(brief) }, describe(selected)));
}
function editableTarget(element) {
  let node = element;
  const root = rootOf(element);
  while (node && node !== root && node.parentElement !== root) {
    const display = getComputedStyle(node).display;
    if (display !== 'inline') { break; }
    node = node.parentElement;
  }
  return node;
}
let pendingHtml = null;
function scheduleHtml(root) {
  clearTimeout(pendingHtml);
  pendingHtml = setTimeout(() => postHtml(root, false), 200);
}
function makeEditable(element) {
  if (!hasOwnText(element)) { return; }
  try { element.contentEditable = 'plaintext-only'; } catch (error) { element.contentEditable = 'true'; }
  if (element.contentEditable !== 'plaintext-only') { element.contentEditable = 'true'; }
  element.spellcheck = false;
  element.focus();
}
function showMenu(x, y, items) {
  menu.innerHTML = '';
  items.forEach((item) => {
    if (item === '-') { menu.appendChild(document.createElement('hr')); return; }
    if (item.color) {
      const label = document.createElement('label');
      const input = document.createElement('input');
      input.type = 'color';
      input.value = item.value || '#000000';
      input.addEventListener('click', (event) => event.stopPropagation());
      input.addEventListener('input', () => item.run(input.value));
      input.addEventListener('change', () => item.done && item.done());
      label.appendChild(input);
      label.appendChild(document.createTextNode(item.label));
      label.addEventListener('click', (event) => event.stopPropagation());
      menu.appendChild(label);
      return;
    }
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = item.label;
    button.addEventListener('click', (event) => { event.stopPropagation(); hideMenu(); item.run(); });
    menu.appendChild(button);
  });
  menu.style.display = 'block';
  const width = menu.offsetWidth, height = menu.offsetHeight;
  menu.style.left = Math.min(x, window.innerWidth - width - 8) + 'px';
  menu.style.top = Math.min(y, window.innerHeight - height - 8) + 'px';
}
function fontSizeOf(element) { return parseFloat(getComputedStyle(element).fontSize) || 32; }
function isPositioned(element) { const position = getComputedStyle(element).position; return position === 'absolute' || position === 'relative' || position === 'fixed'; }
function applyAction(root, element, action, value) {
  switch (action) {
    case 'bigger': element.style.fontSize = Math.round(fontSizeOf(element) * 1.1) + 'px'; break;
    case 'smaller': element.style.fontSize = Math.max(8, Math.round(fontSizeOf(element) / 1.1)) + 'px'; break;
    case 'align': element.style.textAlign = value; break;
    case 'color': {
      const background = toHex(getComputedStyle(element).backgroundColor);
      if (!hasOwnText(element) && !element.querySelector('*') === false && background) { element.style.backgroundColor = value; }
      else if (!hasOwnText(element) && background && !element.textContent.trim()) { element.style.backgroundColor = value; }
      else { element.style.color = value; }
      break;
    }
    case 'forward': if (element.nextElementSibling) { element.parentElement.insertBefore(element.nextElementSibling, element); } break;
    case 'backward': if (element.previousElementSibling) { element.parentElement.insertBefore(element, element.previousElementSibling); } break;
    case 'duplicate': {
      const copy = element.cloneNode(true);
      copy.removeAttribute('data-swift-design-selected');
      if (isPositioned(element)) {
        copy.style.left = ((parseFloat(getComputedStyle(element).left) || 0) + 24) + 'px';
        copy.style.top = ((parseFloat(getComputedStyle(element).top) || 0) + 24) + 'px';
      }
      element.after(copy);
      break;
    }
    case 'delete': {
      if (element.parentElement === root && root.children.length <= 1) { return false; }
      element.remove();
      select(null);
      break;
    }
    case 'background': root.style.background = value; break;
    case 'add-text': {
      const box = document.createElement('div');
      box.setAttribute('style', 'position:absolute;left:120px;top:120px;font-size:40px;');
      box.textContent = 'New text';
      root.appendChild(box);
      select(box);
      break;
    }
    case 'add-image': {
      const image = document.createElement('img');
      image.setAttribute('src', value || '/uploads/image.png');
      image.setAttribute('style', 'position:absolute;left:120px;top:120px;width:480px;');
      root.appendChild(image);
      select(image);
      break;
    }
    case 'text': if (element.children.length === 0) { element.textContent = value; } break;
    case 'font_size': element.style.fontSize = value; break;
    case 'text_color': element.style.color = value; break;
    case 'text_align': element.style.textAlign = value; break;
    case 'padding': element.style.padding = value; break;
    case 'src': element.setAttribute('src', value); break;
    case 'reset-position': element.style.removeProperty('translate'); break;
    case 'select_parent': if (element.parentElement && element.parentElement !== root) { select(element.parentElement); } else { select(root); } return false;
    default: return false;
  }
  return true;
}
const DRAG_THRESHOLD = 4;
let drag = null;
let isClickSuppressed = false;
// The drag offset lives in the standalone `translate` property, so it
// never disturbs the layout of the siblings and never overwrites a
// `transform` the screen CSS already set.
function translateOf(element) {
  const parts = (element.style.translate || '').trim().split(/\s+/);
  return { x: parseFloat(parts[0]) || 0, y: parseFloat(parts[1]) || 0 };
}
function endDrag(save) {
  if (!drag) { return; }
  const finished = drag;
  drag = null;
  if (!finished.moved) { return; }
  finished.root.removeAttribute('data-swift-design-dragging');
  isClickSuppressed = true;
  if (save) { postHtml(finished.root, true); }
}
document.addEventListener('pointermove', (event) => {
  if (!drag) { return; }
  const dx = event.clientX - drag.x;
  const dy = event.clientY - drag.y;
  if (!drag.moved) {
    if (Math.abs(dx) < DRAG_THRESHOLD && Math.abs(dy) < DRAG_THRESHOLD) { return; }
    drag.moved = true;
    drag.root.setAttribute('data-swift-design-dragging', '');
    hideMenu();
    select(drag.element);
  }
  event.preventDefault();
  // Screen pixels divided by the root's scale give canvas pixels.
  const x = Math.round(drag.base.x + dx / drag.scale);
  const y = Math.round(drag.base.y + dy / drag.scale);
  drag.element.style.translate = x + 'px ' + y + 'px';
}, { passive: false });
document.addEventListener('pointerup', () => endDrag(true));
document.addEventListener('pointercancel', () => endDrag(false));
window.addEventListener('blur', () => endDrag(true));
document.querySelectorAll('[data-swift-design-root]').forEach((root) => {
  const screen = screenIndexOf(root);
  root.addEventListener('pointerdown', (event) => {
    if (event.button !== 0 || event.target === root || event.target.isContentEditable) { return; }
    const element = editableTarget(event.target);
    if (!element || element === root) { return; }
    const width = root.getBoundingClientRect().width;
    drag = {
      element, root, moved: false,
      x: event.clientX, y: event.clientY,
      base: translateOf(element),
      scale: width / canvasWidth || 1,
    };
  });
  root.addEventListener('click', (event) => {
    // A click on a link selects it. The page never leaves in edit mode.
    if (event.target.closest && event.target.closest('a[href]')) { event.preventDefault(); }
    if (isClickSuppressed) { isClickSuppressed = false; event.stopPropagation(); return; }
    if (event.target === root) { select(null); post({ type: 'swift-design-select', screen, path: null }); return; }
    event.stopPropagation();
    select(editableTarget(event.target), event.metaKey || event.ctrlKey);
  });
  root.addEventListener('dblclick', (event) => {
    if (event.target === root) { return; }
    event.stopPropagation();
    const target = editableTarget(event.target);
    select(target);
    makeEditable(target);
  });
  root.addEventListener('input', () => scheduleHtml(root));
  root.addEventListener('focusout', (event) => {
    const target = event.target;
    if (target && target.isContentEditable) {
      clearTimeout(pendingHtml);
      target.removeAttribute('contenteditable');
      target.removeAttribute('spellcheck');
      postHtml(root, true);
    }
  });
  root.addEventListener('contextmenu', (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (event.target === root) {
      showMenu(event.clientX, event.clientY, [
        { label: 'Background color', color: true, value: toHex(getComputedStyle(root).backgroundColor) || '#ffffff', run: (value) => { applyAction(root, root, 'background', value); }, done: () => postHtml(root, true) },
        '-',
        { label: 'Add text', run: () => { applyAction(root, root, 'add-text'); postHtml(root, true); } },
        { label: 'Add image', run: () => { applyAction(root, root, 'add-image'); postHtml(root, true); } },
        '-',
        { label: 'Ask AI about this screen', run: () => post({ type: 'swift-design-action', screen, action: 'ask', path: null }) },
        { label: 'Properties…', run: () => post({ type: 'swift-design-action', screen, action: 'properties', path: null }) },
        '-',
        { label: 'Delete screen', run: () => post({ type: 'swift-design-action', screen, action: 'delete-screen', path: null }) },
      ]);
      return;
    }
    const element = editableTarget(event.target);
    select(element);
    const act = (action, value) => () => { if (applyAction(root, element, action, value)) { postHtml(root, true); } };
    const items = [];
    if (hasOwnText(element)) {
      items.push({ label: 'Bigger', run: act('bigger') });
      items.push({ label: 'Smaller', run: act('smaller') });
      items.push({ label: 'Align left', run: act('align', 'left') });
      items.push({ label: 'Align center', run: act('align', 'center') });
      items.push({ label: 'Align right', run: act('align', 'right') });
      items.push('-');
    }
    items.push({ label: 'Color', color: true, value: toHex(getComputedStyle(element).color) || '#000000', run: (value) => { applyAction(root, element, 'color', value); }, done: () => postHtml(root, true) });
    items.push('-');
    if (element.style.translate) { items.push({ label: 'Reset position', run: act('reset-position') }); }
    items.push({ label: 'Move forward', run: act('forward') });
    items.push({ label: 'Move backward', run: act('backward') });
    items.push({ label: 'Duplicate', run: act('duplicate') });
    items.push('-');
    items.push({ label: 'Ask AI about this', run: () => post(Object.assign({ type: 'swift-design-action', screen, action: 'ask' }, describe(element))) });
    items.push({ label: 'Properties…', run: () => post(Object.assign({ type: 'swift-design-action', screen, action: 'properties' }, describe(element))) });
    items.push('-');
    items.push({ label: 'Delete', run: act('delete') });
    showMenu(event.clientX, event.clientY, items);
  });
});
window.addEventListener('message', (event) => {
  if (event.origin !== origin || event.source !== window.parent) { return; }
  const data = event.data;
  if (!data || data.type !== 'swift-design-apply') { return; }
  const section = document.querySelector('[data-swift-design-screen="' + Number(data.screen) + '"]');
  const root = section && section.querySelector('[data-swift-design-root]');
  if (!root) { return; }
  const element = nodeAt(root, data.path);
  if (!element) { return; }
  if (applyAction(root, element, data.property, data.value)) { postHtml(root, false); }
});
"##;

/// The layout audit for the polish pass. Loaded only with
/// `is_auditing`. After fonts load, it measures every screen and writes
/// the findings as JSON into `data-swift-design-findings` on `<html>`, so
/// a DOM dump carries them back to the server. `polish.rs` reads the
/// findings and names the kinds.
pub(crate) const AUDIT_SCRIPT: &str = r##"(async () => {
  if (document.readyState !== 'complete') {
    await new Promise((resolve) => window.addEventListener('load', resolve, { once: true }));
  }
  if (document.fonts && document.fonts.ready) { try { await document.fonts.ready; } catch (error) {} }
  const findings = [];
  const hasOwnText = (element) => Array.from(element.childNodes).some((node) => node.nodeType === 3 && node.textContent.trim());
  document.querySelectorAll('[data-swift-design-root]').forEach((root) => {
    const screen = Number(root.closest('[data-swift-design-screen]').dataset.swiftDesignScreen);
    const rootRect = root.getBoundingClientRect();
    const scale = rootRect.width / canvasWidth || 1;
    const pathOf = (element) => {
      const parts = [];
      let node = element;
      while (node && node !== root) { const parent = node.parentElement; if (!parent) { break; } parts.unshift(Array.prototype.indexOf.call(parent.children, node)); node = parent; }
      return parts.join('/');
    };
    const name = (element) => element.tagName.toLowerCase() + (element.classList[0] ? '.' + element.classList[0] : '') + ' (' + pathOf(element) + ')';
    if (!root.textContent.trim() && !root.querySelector('img, svg')) {
      findings.push({ screen, node: 'root', kind: 'empty', detail: 'the screen has no text, image, or svg' });
    }
    const all = Array.from(root.querySelectorAll('*')).filter((element) => {
      const style = getComputedStyle(element);
      return style.display !== 'none' && style.visibility !== 'hidden';
    });
    const overflowing = all.filter((element) => element.clientHeight > 0 && element.scrollHeight > element.clientHeight + 2 && !element.closest('svg'));
    overflowing.forEach((element) => {
      if (overflowing.some((other) => other !== element && element.contains(other))) { return; }
      findings.push({ screen, node: name(element), kind: 'overflow', detail: 'content needs ' + Math.round(element.scrollHeight) + 'px but the box is ' + Math.round(element.clientHeight) + 'px tall' });
    });
    if (root.scrollHeight > root.clientHeight + 2 || root.scrollWidth > root.clientWidth + 2) {
      findings.push({ screen, node: 'root', kind: 'off_screen', detail: 'content runs past the ' + canvasWidth + ' by ' + canvasHeight + ' canvas (' + Math.round(root.scrollWidth) + ' by ' + Math.round(root.scrollHeight) + 'px)' });
    }
    // The page shrinks a screen that holds too much, so it is whole
    // rather than cut off. It is still too much content.
    const fitFactor = Number(root.dataset.swiftDesignFit || 1);
    if (fitFactor < 0.97) {
      findings.push({ screen, node: 'root', kind: 'overfull', detail: 'the screen holds more than the ' + canvasWidth + ' by ' + canvasHeight + ' canvas fits, so it was scaled to ' + Math.round(fitFactor * 100) + '%: cut content or reduce the sizes' });
    }
    // The smallest readable text depends on the canvas. A slide is
    // read from across a room; a desktop app screen uses 12 to 14px
    // labels, and a phone screen 11 to 12px. The deck floor applied
    // to a demo flagged every label, and the real layout findings
    // were lost in hundreds of these.
    const textFloor = canvasWidth >= 1920 ? { flag: 20, ask: 24 } : canvasWidth >= 1000 ? { flag: 12, ask: 14 } : { flag: 11, ask: 12 };
    const textBlocks = [];
    all.forEach((element) => {
      if (element.closest('svg') && element.tagName.toLowerCase() !== 'svg') { return; }
      const isText = hasOwnText(element);
      const tag = element.tagName.toLowerCase();
      if (!isText && tag !== 'img' && tag !== 'svg') { return; }
      const rect = element.getBoundingClientRect();
      const tolerance = 2 * scale;
      if (rect.left < rootRect.left - tolerance || rect.top < rootRect.top - tolerance || rect.right > rootRect.right + tolerance || rect.bottom > rootRect.bottom + tolerance) {
        findings.push({ screen, node: name(element), kind: 'off_screen', detail: 'runs off the screen: keep it inside the ' + canvasWidth + ' by ' + canvasHeight + ' canvas' });
      }
      if (isText) {
        const size = parseFloat(getComputedStyle(element).fontSize);
        if (size < textFloor.flag) {
          findings.push({ screen, node: name(element), kind: 'tiny_text', detail: 'font-size ' + Math.round(size) + 'px is too small: use at least ' + textFloor.ask + 'px' });
        }
        textBlocks.push({ element, rect });
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
})();
"##;

/// How to render a design page.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Nodes are selectable and text is editable in place; every change
    /// is posted to the parent window for the editor.
    pub is_editable: bool,
    /// Render only this zero-based screen. Used by thumbnails and the
    /// editor preview, so the page never scrolls.
    pub only_screen: Option<usize>,
    /// Adds the layout audit script; the polish pass reads its result
    /// from a DOM dump.
    pub is_auditing: bool,
    /// Extra origin allowed for images, such as the server URL when the
    /// page loads from a file for a screenshot.
    pub asset_origin: Option<String>,
    /// One screen per viewport-sized page, no scripts, no transition.
    /// Used for `--print-to-pdf`.
    pub is_print: bool,
}

/// Print rules appended after the base stylesheet, so they override it.
/// Every frame is one page the size of the canvas, so the root needs no
/// scaling at all.
pub(crate) fn print_stylesheet(viewport: Viewport) -> String {
    let width = viewport.width;
    let height = viewport.height;
    format!(
        "@page {{ size: {width}px {height}px; margin: 0; }}\n\
         html, body {{ height: auto; background: transparent; }}\n\
         * {{ -webkit-print-color-adjust: exact; print-color-adjust: exact; }}\n\
         main.design {{ height: auto; overflow: visible; scroll-snap-type: none; }}\n\
         main.design > [data-swift-design-frame] {{ width: {width}px; height: {height}px; display: block; overflow: hidden; }}\n\
         main.design > [data-swift-design-frame]:not(:last-child) {{ break-after: page; page-break-after: always; }}\n\
         [data-swift-design-screen] {{ width: {width}px; height: {height}px; container-type: normal; --swift-design-scale: 1; }}\n\
         [data-swift-design-root] {{ transform-origin: 0 0;\n\
           transform: scale(var(--swift-design-fit, 1)); }}\n"
    )
}

/// Renders the whole design as an HTML document.
pub fn render_design(design: &Design, is_editable: bool) -> String {
    render_design_with(
        design,
        RenderOptions {
            is_editable,
            ..RenderOptions::default()
        },
    )
}

/// Renders the design, or one screen of it, per `options`.
pub fn render_design_with(design: &Design, options: RenderOptions) -> String {
    let mut sections = String::new();
    let mut screen_styles = String::new();
    for (index, screen) in design.screens.iter().enumerate() {
        if options.only_screen.is_some_and(|only| only != index) {
            continue;
        }
        sections.push_str(&render_screen(screen, index));
        if let Some(css) = &screen.css {
            screen_styles.push_str(&scope_css(
                css,
                &format!("[data-swift-design-screen=\"{index}\"]"),
            ));
            screen_styles.push('\n');
        }
    }
    let script = page_script(&options);
    let (script_source, script_element) = script_markup(&script);
    // A single-screen page has nothing to transition to, so the editor
    // preview, thumbnails, and screenshots keep the plain scroll page.
    // A print has one screen per page, so it never transitions either.
    let transition = (options.only_screen.is_none() && !options.is_print)
        .then_some(design.transition)
        .flatten();
    let design_attributes = transition.map(design_attributes).unwrap_or_default();
    let transition_style = transition.map(transition_styles).unwrap_or_default();
    let print_style = if options.is_print {
        print_stylesheet(design.viewport)
    } else {
        String::new()
    };
    let image_sources = match &options.asset_origin {
        Some(origin) => format!("'self' data: {}", css_safe(origin).replace(' ', "")),
        None => "'self' data:".to_owned(),
    };
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; \
         script-src {script_source}; style-src 'unsafe-inline' https://fonts.googleapis.com; \
         font-src 'self' https://fonts.gstatic.com; img-src {image_sources}; connect-src 'none'; \
         object-src 'none'; frame-src 'none'; form-action 'none'\">\n\
         <title>{title}</title>\n{fonts}<style>\n{style}{print_style}{transition_style}{screen_styles}</style>\n</head>\n<body>\n\
         <main class=\"design\" data-swift-design-width=\"{width}\" data-swift-design-height=\"{height}\"{design_attributes}>\n{sections}</main>\n\
         {script_element}</body>\n</html>\n",
        title = escape_html(&design.title),
        fonts = google_fonts_link(&design.theme).unwrap_or_default(),
        style = stylesheet(&design.theme, design.viewport),
        width = design.viewport.width,
        height = design.viewport.height,
    )
}

/// The page script for `options`: the fit always, layout and navigation
/// on screen, editing and audit on request.
///
/// A print carries the fit and nothing else: the PDF must hold the same
/// content the studio shows, and the rest of the scripts have no place
/// in a print.
fn page_script(options: &RenderOptions) -> String {
    if options.is_print {
        return FIT_SCRIPT.to_owned();
    }
    let mut script = FIT_SCRIPT.to_owned();
    script.push_str(LAYOUT_SCRIPT);
    script.push_str(NAVIGATION_SCRIPT);
    if options.is_editable {
        script.push_str(EDITING_SCRIPT);
    }
    if options.is_auditing {
        script.push_str(AUDIT_SCRIPT);
    }
    script
}

/// The `script-src` value and the `<script>` element for `script`. An
/// empty script allows no script at all and emits no element.
pub(crate) fn script_markup(script: &str) -> (String, String) {
    if script.is_empty() {
        return ("'none'".to_owned(), String::new());
    }
    (
        format!("'sha256-{}'", script_hash(script)),
        format!("<script>{script}</script>\n"),
    )
}

/// The `main.design` attributes that name the design's transition. The
/// navigation script reads the duration; the CSS reads the effect and
/// the axis.
pub(crate) fn design_attributes(transition: Transition) -> String {
    format!(
        " data-swift-design-effect=\"{effect}\" data-swift-design-axis=\"{axis}\" \
         data-swift-design-duration=\"{duration}\"",
        effect = transition.effect.as_str(),
        axis = transition.axis.as_str(),
        duration = transition_ms(transition),
    )
}

/// The transition duration the page uses: the design's value, capped, and
/// zero for `none`, which always cuts.
fn transition_ms(transition: Transition) -> u32 {
    match transition.effect {
        TransitionEffect::None => 0,
        _ => transition.duration_ms.min(MAX_TRANSITION_MS),
    }
}

/// The stacked-layout CSS for a transition.
///
/// Every frame sits on top of the others and stays hidden until it
/// carries a `data-swift-design-state`. The script moves a frame through
/// `entering` then `current`, which animates it from the effect's start
/// state to none, and marks the old frame `leaving`.
pub(crate) fn transition_styles(transition: Transition) -> String {
    let frame = "main.design[data-swift-design-effect] > [data-swift-design-frame]";
    let motion = "transition: opacity var(--swift-design-duration) ease, \
                  transform var(--swift-design-duration) ease;";
    let (entering, leaving) = match transition.effect {
        TransitionEffect::None => ("opacity: 0;", "opacity: 0;"),
        TransitionEffect::Fade => ("opacity: 0;", "opacity: 0;"),
        TransitionEffect::Push => (
            "opacity: 1; transform: var(--swift-design-in);",
            "opacity: 1; transform: var(--swift-design-out);",
        ),
        TransitionEffect::Cover => (
            "opacity: 1; transform: var(--swift-design-in);",
            "opacity: 1; transform: none;",
        ),
        TransitionEffect::Zoom => (
            "opacity: 0; transform: scale(0.88);",
            "opacity: 0; transform: scale(1.12);",
        ),
    };
    format!(
        "main.design[data-swift-design-effect] {{ position: relative; height: 100vh; overflow: hidden;\n\
           scroll-snap-type: none; --swift-design-sign: 1; --swift-design-duration: {duration}ms; }}\n\
         main.design[data-swift-design-axis=\"vertical\"] {{\n\
           --swift-design-in: translate3d(0, calc(var(--swift-design-sign) * 100%), 0);\n\
           --swift-design-out: translate3d(0, calc(var(--swift-design-sign) * -100%), 0); }}\n\
         main.design[data-swift-design-axis=\"horizontal\"] {{\n\
           --swift-design-in: translate3d(calc(var(--swift-design-sign) * 100%), 0, 0);\n\
           --swift-design-out: translate3d(calc(var(--swift-design-sign) * -100%), 0, 0); }}\n\
         {frame} {{ position: absolute; inset: 0; height: 100%; opacity: 0; visibility: hidden;\n\
           z-index: 0; will-change: opacity, transform; }}\n\
         {frame}[data-swift-design-state] {{ visibility: visible; }}\n\
         {frame}[data-swift-design-state=\"entering\"] {{ z-index: 2; {entering} }}\n\
         {frame}[data-swift-design-state=\"leaving\"] {{ z-index: 1; {leaving} {motion} }}\n\
         {frame}[data-swift-design-state=\"current\"] {{ z-index: 2; opacity: 1; transform: none; {motion} }}\n\
         @media (prefers-reduced-motion: reduce) {{\n\
           main.design[data-swift-design-effect] {{ --swift-design-duration: 0ms; }} }}\n",
        duration = transition_ms(transition),
    )
}

/// The base64 SHA-256 of the page script, for the CSP.
pub(crate) fn script_hash(script: &str) -> String {
    base64_encode(&Sha256::digest(script.as_bytes()))
}

/// Renders one screen: a full-window frame that centers the viewport
/// box, and the viewport-sized root that holds the screen's HTML as
/// written.
fn render_screen(screen: &Screen, index: usize) -> String {
    format!(
        "<div class=\"screen-frame\" id=\"screen-{number}\" data-swift-design-frame>\n\
         <section class=\"screen\" data-swift-design-screen=\"{index}\">\n\
         <div class=\"screen-root\" data-swift-design-root>{html}</div>\n\
         </section>\n</div>\n",
        number = index + 1,
        html = screen.html,
    )
}

/// Builds the base stylesheet: page chrome, the viewport-shaped frame,
/// the scaled viewport-sized root, theme variables, and small defaults
/// that screen CSS always overrides.
pub(crate) fn stylesheet(theme: &Theme, viewport: Viewport) -> String {
    let background = css_safe(&theme.colors.background);
    let text = css_safe(&theme.colors.text);
    let accent = css_safe(&theme.colors.accent);
    let muted = css_safe(&theme.colors.muted);
    let heading_font = css_safe(&theme.fonts.heading);
    let body_font = css_safe(&theme.fonts.body);
    let mono_font = css_safe(&theme.fonts.mono);
    let width = viewport.width;
    let height = viewport.height;
    format!(
        "html, body {{ margin: 0; height: 100%; background: #000; }}\n\
         main.design {{ height: 100vh; overflow-y: auto; scroll-snap-type: y mandatory; }}\n\
         main.design > [data-swift-design-frame] {{ height: 100vh; display: flex; align-items: center;\n\
           justify-content: center; scroll-snap-align: start; }}\n\
         [data-swift-design-screen] {{ position: relative; width: min(100vw, calc(100vh * {width} / {height}));\n\
           aspect-ratio: {width} / {height};\n\
           container-type: size; contain: layout paint; overflow: hidden; box-sizing: border-box;\n\
           background: {background};\n\
           --swift-design-scale: calc(tan(atan2(100cqw, {width}px)));\n\
           --background: {background}; --text: {text}; --accent: {accent}; --muted: {muted};\n\
           --heading-font: '{heading_font}', Inter, system-ui, sans-serif;\n\
           --body-font: '{body_font}', Inter, system-ui, sans-serif;\n\
           --mono-font: '{mono_font}', ui-monospace, monospace; }}\n\
         [data-swift-design-root] {{ position: relative; width: {width}px; height: {height}px; overflow: hidden;\n\
           box-sizing: border-box; transform-origin: 0 0;\n\
           transform: scale(calc(var(--swift-design-scale, 1) * var(--swift-design-fit, 1)));\n\
           background: var(--background); color: var(--text);\n\
           font: 32px/1.3 var(--body-font); }}\n\
         [data-swift-design-root] * {{ box-sizing: border-box; }}\n\
         [data-swift-design-inner-root] {{ width: 100% !important; height: 100% !important;\n\
           min-height: 0 !important; max-height: none !important; max-width: none !important; }}\n\
         [data-swift-design-root] :is(h1, h2, h3, h4, h5, h6) {{ font-family: var(--heading-font);\n\
           line-height: 1.1; margin: 0; letter-spacing: -0.02em; }}\n\
         [data-swift-design-root] :is(p, ul, ol, figure, blockquote) {{ margin: 0; }}\n\
         [data-swift-design-root] :is(ul, ol) {{ padding-left: 1.2em; }}\n\
         [data-swift-design-root] img {{ display: block; max-width: 100%; }}\n\
         [data-swift-design-root] :is(pre, code) {{ font-family: var(--mono-font); }}\n\
         [data-swift-design-root] a {{ color: var(--accent); }}\n\
         [data-swift-design-root] table {{ border-collapse: collapse; }}\n"
    )
}

/// Escapes text for HTML element and attribute positions.
pub(crate) fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Keeps only characters that cannot break out of a CSS declaration or
/// a CSP directive. Theme values come from agent-written JSON, so
/// quotes, braces, semicolons, and `</style>` sequences must never reach
/// the stylesheet.
pub(crate) fn css_safe(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, ' ' | '-' | '_' | '#' | ',' | ':' | '/' | '.')
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::{Design, Transition, TransitionAxis, TransitionEffect};
    use sha2::Digest;

    use crate::export::base64_encode;
    use crate::render::{
        EDITING_SCRIPT, FIT_SCRIPT, MAX_TRANSITION_MS, NAVIGATION_SCRIPT, RenderOptions, css_safe,
        escape_html, render_design, render_design_with,
    };

    fn sample_design() -> Design {
        serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap()
    }

    #[test]
    fn a_screen_link_opens_the_screen_in_play_mode_and_only_selects_in_edit_mode() {
        assert!(NAVIGATION_SCRIPT.contains("/^#screen-(\\d+)$/"));
        assert!(NAVIGATION_SCRIPT.contains("type: 'swift-design-navigate', target"));
        assert!(EDITING_SCRIPT.contains("closest('a[href]')) { event.preventDefault(); }"));
        // The frame ids are the link targets, so a full page scrolls to
        // them by itself.
        let html = render_design(&sample_design(), false);
        assert!(html.contains("id=\"screen-2\""));
    }

    #[test]
    fn renders_one_section_per_screen_with_the_html_as_written() {
        let design = sample_design();
        let html = render_design(&design, false);
        assert_eq!(
            html.matches("<div class=\"screen-root\" data-swift-design-root>")
                .count(),
            design.screens.len()
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("id=\"screen-1\""));
        assert!(html.contains("<h1>Swift Design</h1>"));
        assert!(html.contains("<ul><li>Agents read the schema and write design JSON.</li>"));
    }

    #[test]
    fn screen_css_is_scoped_to_its_screen() {
        let html = render_design(&sample_design(), false);
        assert!(html.contains("[data-swift-design-screen=\"0\"] .s1-hero{"));
        assert!(html.contains("[data-swift-design-screen=\"2\"] h2{"));
        assert!(!html.contains("\nh2{"));
    }

    #[test]
    fn theme_variables_fonts_and_scaling_are_present() {
        let html = render_design(&sample_design(), false);
        assert!(html.contains("--background: #101418"));
        assert!(html.contains("--heading-font: 'Inter'"));
        assert!(html.contains("--mono-font: 'JetBrains Mono'"));
        assert!(html.contains("fonts.googleapis.com/css2?family=Inter"));
        assert!(html.contains("--swift-design-scale: calc(tan(atan2(100cqw, 1440px)))"));
        assert!(html.contains("aspect-ratio: 1440 / 900;"));
        assert!(html.contains("data-swift-design-width=\"1440\" data-swift-design-height=\"900\""));
        // The page scale and the content fit multiply into one transform.
        assert!(html.contains(
            "transform: scale(calc(var(--swift-design-scale, 1) * var(--swift-design-fit, 1)))"
        ));
        assert!(html.contains("new ResizeObserver"));
    }

    #[test]
    fn the_csp_hash_matches_the_emitted_script() {
        let html = render_design(&sample_design(), true);
        let start = html.find("<script>").unwrap() + "<script>".len();
        let end = html.find("</script>").unwrap();
        let script = &html[start..end];
        let hash = base64_encode(&sha2::Sha256::digest(script.as_bytes()));
        assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
        assert!(html.contains("connect-src 'none'"));
        assert!(html.contains("img-src 'self' data:;"));
    }

    #[test]
    fn a_single_screen_renders_with_its_real_index() {
        let html = render_design_with(
            &sample_design(),
            RenderOptions {
                is_editable: true,
                only_screen: Some(2),
                ..RenderOptions::default()
            },
        );
        assert_eq!(
            html.matches("<section class=\"screen\" data-swift-design-screen=\"")
                .count(),
            1
        );
        assert!(html.contains("<section class=\"screen\" data-swift-design-screen=\"2\">"));
        assert!(html.contains("id=\"screen-3\""));
        // Only that screen's CSS is emitted.
        assert!(html.contains("[data-swift-design-screen=\"2\"] h2{"));
        assert!(!html.contains("[data-swift-design-screen=\"0\"]"));
    }

    #[test]
    fn scripts_follow_the_options() {
        let design = sample_design();
        let plain = render_design(&design, false);
        assert!(!plain.contains("swift-design-html"));
        assert!(!plain.contains("data-swift-design-findings"));
        assert!(!plain.contains("data-swift-design-dragging"));
        let editable = render_design(&design, true);
        assert!(editable.contains("swift-design-html"));
        assert!(editable.contains("swift-design-apply"));
        assert!(editable.contains("contextmenu"));
        assert!(editable.contains("contentEditable"));
        // Dragging moves a node; a double click is what edits its text.
        assert!(editable.contains("data-swift-design-dragging"));
        assert!(editable.contains("style.translate"));
        assert!(editable.contains("'dblclick'"));
        // The HTML posted back carries none of the scripts' marks.
        assert!(editable.contains("node.removeAttribute('data-swift-design-inner-root')"));
        // A command-click adds a node to the selection.
        assert!(editable.contains("event.metaKey || event.ctrlKey"));
        assert!(editable.contains("selection: selection.map(brief)"));
        assert!(editable.contains("reset-position"));
        let auditing = render_design_with(
            &design,
            RenderOptions {
                is_auditing: true,
                asset_origin: Some("http://127.0.0.1:3000".to_owned()),
                ..RenderOptions::default()
            },
        );
        assert!(auditing.contains("data-swift-design-findings"));
        assert!(auditing.contains("img-src 'self' data: http://127.0.0.1:3000;"));
    }

    #[test]
    fn every_page_carries_the_fit_script() {
        // A screen that holds more than the canvas would be cut off, so
        // the fit runs on the studio page, the audit page, and the print
        // page alike.
        let design = sample_design();
        for options in [
            RenderOptions::default(),
            RenderOptions {
                is_editable: true,
                ..RenderOptions::default()
            },
            RenderOptions {
                is_auditing: true,
                ..RenderOptions::default()
            },
            RenderOptions {
                is_print: true,
                ..RenderOptions::default()
            },
        ] {
            let html = render_design_with(&design, options);
            assert!(html.contains("--swift-design-fit"), "{html}");
        }
    }

    #[test]
    fn the_fit_measures_through_an_agents_own_root() {
        // An agent's own canvas-sized, clipped box hides its overflow
        // from the root: the script marks it, the stylesheet sizes it
        // with the root, and the text inside is measured directly.
        assert!(FIT_SCRIPT.contains("swiftDesignInnerRoot"));
        assert!(FIT_SCRIPT.contains("contentOverflows(root)"));
        let html = render_design(&sample_design(), false);
        assert!(html.contains(
            "[data-swift-design-inner-root] { width: 100% !important; height: 100% !important;"
        ));
    }

    #[test]
    fn print_mode_emits_page_rules_and_the_fit_script_only() {
        let mut design = sample_design();
        design.transition = Some(Transition::default());
        let html = render_design_with(
            &design,
            RenderOptions {
                is_print: true,
                ..RenderOptions::default()
            },
        );
        assert!(html.contains("@page { size: 1440px 900px; margin: 0; }"));
        assert!(html.contains("break-after: page"));
        assert!(html.contains("transform: scale(var(--swift-design-fit, 1)); }"));
        // A print carries the fit and nothing else, so the PDF holds the
        // same content the studio shows.
        assert!(html.contains("swiftDesignFit"));
        assert!(!html.contains("ResizeObserver"));
        assert!(!html.contains("addEventListener('keydown'"));
        assert!(!html.contains("ResizeObserver"));
        assert!(!html.contains("ArrowRight"));
        assert!(!html.contains("data-swift-design-effect="));
        assert!(html.contains("fonts.googleapis.com"));
        assert_eq!(
            html.matches("data-swift-design-root>").count(),
            design.screens.len()
        );
    }

    #[test]
    fn a_design_without_a_transition_renders_the_plain_scroll_page() {
        let html = render_design(&sample_design(), false);
        assert!(!html.contains("data-swift-design-effect="));
        assert!(!html.contains("--swift-design-in"));
        assert!(html.contains(
            "<main class=\"design\" data-swift-design-width=\"1440\" data-swift-design-height=\"900\">"
        ));
        assert!(html.contains("scroll-snap-type: y mandatory"));
    }

    #[test]
    fn a_transition_adds_the_design_attributes_and_the_stacked_rules() {
        let mut design = sample_design();
        design.transition = Some(Transition {
            effect: TransitionEffect::Cover,
            axis: TransitionAxis::Horizontal,
            duration_ms: 620,
        });
        let html = render_design(&design, false);
        assert!(html.contains("data-swift-design-effect=\"cover\""));
        assert!(html.contains("data-swift-design-axis=\"horizontal\""));
        assert!(html.contains("data-swift-design-duration=\"620\""));
        assert!(html.contains("--swift-design-duration: 620ms"));
        // Cover moves the new screen in and leaves the old one still.
        assert!(html.contains("[data-swift-design-state=\"entering\"] { z-index: 2; opacity: 1; transform: var(--swift-design-in); }"));
        assert!(html.contains(
            "[data-swift-design-state=\"leaving\"] { z-index: 1; opacity: 1; transform: none;"
        ));
        assert!(html.contains("prefers-reduced-motion"));
    }

    #[test]
    fn the_none_effect_cuts_whatever_duration_the_design_asks_for() {
        let mut design = sample_design();
        design.transition = Some(Transition {
            effect: TransitionEffect::None,
            duration_ms: 900,
            ..Transition::default()
        });
        let html = render_design(&design, false);
        assert!(html.contains("data-swift-design-duration=\"0\""));
        assert!(html.contains("--swift-design-duration: 0ms"));
    }

    #[test]
    fn a_duration_over_the_limit_is_capped_in_the_page() {
        let mut design = sample_design();
        design.transition = Some(Transition {
            duration_ms: 99_000,
            ..Transition::default()
        });
        let html = render_design(&design, false);
        assert!(html.contains(&format!(
            "data-swift-design-duration=\"{MAX_TRANSITION_MS}\""
        )));
    }

    #[test]
    fn a_single_screen_page_keeps_the_scroll_layout_despite_a_transition() {
        let mut design = sample_design();
        design.transition = Some(Transition::default());
        let html = render_design_with(
            &design,
            RenderOptions {
                only_screen: Some(1),
                ..RenderOptions::default()
            },
        );
        assert!(!html.contains("data-swift-design-effect="));
        assert!(!html.contains("--swift-design-in"));
    }

    #[test]
    fn escapes_the_design_title() {
        let mut design = sample_design();
        design.title = "<script>alert(1)</script>".to_owned();
        let html = render_design(&design, false);
        assert!(html.contains("<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>"));
    }

    #[test]
    fn css_safe_strips_breakout_characters() {
        assert_eq!(css_safe("Inter, sans-serif"), "Inter, sans-serif");
        assert_eq!(css_safe("x'; } </style>"), "x  /style");
        assert_eq!(css_safe("http://127.0.0.1:3000"), "http://127.0.0.1:3000");
    }

    #[test]
    fn escape_html_covers_every_special_character() {
        assert_eq!(
            escape_html(r#"<a href="x">&'"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }
}
