/// In-place editing for the editor preview. Loaded only with
/// `editable=true`, so plain render and export output stays inert.
/// Clicking a node selects it and posts its path; dragging a node moves
/// it; dragging a handle of the selection's bounding box resizes it;
/// double-clicking a text node edits it in place; a right-click menu
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
.swift-design-menu button[data-swift-design-danger] { color: #C0392B; }
.swift-design-menu button[data-swift-design-danger]:hover { background: #FBEDEB; }
.swift-design-menu hr { border: 0; border-top: 1px solid #EAE7E1; margin: 0.25rem 0; }
.swift-design-menu label { display: flex; align-items: center; gap: 0.6rem;
  padding: 0.3rem 0.7rem; cursor: pointer; }
.swift-design-menu input[type='color'] { width: 1.8rem; height: 1.4rem; border: 1px solid #DAD7D0;
  border-radius: 4px; padding: 0; background: #FFFFFF; }
.swift-design-handles { position: fixed; z-index: 9; display: none; pointer-events: none;
  outline: 1px solid rgba(14, 110, 99, 0.9); }
.swift-design-handle { position: absolute; width: 10px; height: 10px; margin: -5px;
  background: #FFFFFF; border: 1px solid #0E6E63; border-radius: 2px; pointer-events: auto; }
[data-swift-design-handle='nw'] { left: 0; top: 0; cursor: nwse-resize; }
[data-swift-design-handle='n'] { left: 50%; top: 0; cursor: ns-resize; }
[data-swift-design-handle='ne'] { left: 100%; top: 0; cursor: nesw-resize; }
[data-swift-design-handle='e'] { left: 100%; top: 50%; cursor: ew-resize; }
[data-swift-design-handle='se'] { left: 100%; top: 100%; cursor: nwse-resize; }
[data-swift-design-handle='s'] { left: 50%; top: 100%; cursor: ns-resize; }
[data-swift-design-handle='sw'] { left: 0; top: 100%; cursor: nesw-resize; }
[data-swift-design-handle='w'] { left: 0; top: 50%; cursor: ew-resize; }
.swift-design-rotate { position: absolute; left: 50%; top: -24px; width: 12px; height: 12px;
  margin: -6px; background: #FFFFFF; border: 1px solid #0E6E63; border-radius: 50%;
  pointer-events: auto; cursor: grab; }
.swift-design-guide { position: fixed; z-index: 8; display: none; pointer-events: none; background: #E0457B; }
.swift-design-guide[data-swift-design-axis='x'] { width: 1px; top: 0; bottom: 0; }
.swift-design-guide[data-swift-design-axis='y'] { height: 1px; left: 0; right: 0; }
`;
document.head.appendChild(editingStyle);
const menu = document.createElement('div');
menu.className = 'swift-design-menu';
document.body.appendChild(menu);
// The bounding box with the resize handles. It lives on the body, like
// the menu, so it never enters the serialized screen HTML.
const handleBox = document.createElement('div');
handleBox.className = 'swift-design-handles';
['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'].forEach((name) => {
  const handle = document.createElement('div');
  handle.className = 'swift-design-handle';
  handle.setAttribute('data-swift-design-handle', name);
  handle.addEventListener('pointerdown', (event) => startResize(event, name));
  handleBox.appendChild(handle);
});
const rotateHandle = document.createElement('div');
rotateHandle.className = 'swift-design-rotate';
rotateHandle.addEventListener('pointerdown', (event) => startRotate(event));
handleBox.appendChild(rotateHandle);
document.body.appendChild(handleBox);
// One vertical and one horizontal guide line for snapping, on the
// body like the menu and the handles.
const guides = {};
['x', 'y'].forEach((axis) => {
  const guide = document.createElement('div');
  guide.className = 'swift-design-guide';
  guide.setAttribute('data-swift-design-axis', axis);
  document.body.appendChild(guide);
  guides[axis] = guide;
});
function setGuide(axis, position) {
  const guide = guides[axis];
  if (position === null) { guide.style.display = 'none'; return; }
  guide.style.display = 'block';
  if (axis === 'x') { guide.style.left = position + 'px'; } else { guide.style.top = position + 'px'; }
}
function hideGuides() { setGuide('x', null); setGuide('y', null); }
function hideMenu() { menu.style.display = 'none'; menu.innerHTML = ''; updateHandles(); }
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
  clone.querySelectorAll('[contenteditable], [spellcheck], [data-swift-design-selected], [data-swift-design-inner-root], [data-swift-design-dragging], [data-swift-design-window-applied]').forEach((node) => {
    node.removeAttribute('contenteditable');
    node.removeAttribute('spellcheck');
    node.removeAttribute('data-swift-design-selected');
    node.removeAttribute('data-swift-design-inner-root');
    node.removeAttribute('data-swift-design-dragging');
    if (node.hasAttribute('data-swift-design-window-applied')) {
      let original = [];
      try { original = JSON.parse(node.getAttribute('data-swift-design-window-applied')); } catch (error) {}
      ['position', 'left', 'top', 'right', 'bottom', 'width', 'height', 'min-width', 'max-width', 'min-height', 'max-height', 'margin', 'box-sizing', 'transform', 'translate', 'rotate', 'scale'].forEach((property) => node.style.removeProperty(property));
      original.forEach(([property, value, priority]) => { if (value) { node.style.setProperty(property, value, priority); } });
      node.removeAttribute('data-swift-design-window-applied');
      if (!node.getAttribute('style')) { node.removeAttribute('style'); }
    }
  });
  return clone.innerHTML;
}
function postHtml(root, save) {
  // An edit changes how much the screen holds, so the fit is measured
  // again before the change leaves the page.
  if (window.swiftDesignFit) { window.swiftDesignFit(); }
  post({ type: 'swift-design-html', screen: screenIndexOf(root), html: serialize(root), save: !!save });
  updateHandles();
}
// Undo keeps one snapshot stack per screen root. A snapshot is the
// serialized HTML from before a mutation; redo holds what undo popped.
const UNDO_LIMIT = 50;
const undoStacks = new Map();
let lastEditedRoot = null;
let coalesced = null;
function stacksOf(root) {
  if (!undoStacks.has(root)) { undoStacks.set(root, { undo: [], redo: [] }); }
  return undoStacks.get(root);
}
function recordSnapshot(root) {
  const stacks = stacksOf(root);
  stacks.undo.push(serialize(root));
  if (stacks.undo.length > UNDO_LIMIT) { stacks.undo.shift(); }
  stacks.redo = [];
  lastEditedRoot = root;
  coalesced = null;
}
// A keystroke stream from the inspector arrives as one apply message
// per key. One snapshot per second per property keeps the stack from
// draining on a burst.
function recordCoalesced(root, key) {
  const now = Date.now();
  if (coalesced && coalesced.key === key && now - coalesced.at < 1000) { coalesced.at = now; return; }
  recordSnapshot(root);
  coalesced = { key, at: now };
}
function undoTargetRoot() {
  return (selected && selected.isConnected && rootOf(selected)) || lastEditedRoot;
}
function restoreSnapshot(pop, push) {
  const root = undoTargetRoot();
  if (!root) { return; }
  const stacks = stacksOf(root);
  const html = stacks[pop].pop();
  if (html === undefined) { return; }
  stacks[push].push(serialize(root));
  root.innerHTML = html;
  select(null);
  postHtml(root, true);
  lastEditedRoot = root;
}
function undo() { restoreSnapshot('undo', 'redo'); }
function redo() { restoreSnapshot('redo', 'undo'); }
// Capture phase, so the shortcut wins over every bubble listener. A
// text edit keeps the browser's own undo.
document.addEventListener('keydown', (event) => {
  if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== 'z') { return; }
  if (event.target && event.target.isContentEditable) { return; }
  event.preventDefault();
  event.stopImmediatePropagation();
  if (event.shiftKey) { redo(); } else { undo(); }
}, true);
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
      width: Math.round(baseSizeOf(element).width) + 'px',
      height: Math.round(baseSizeOf(element).height) + 'px',
      is_leaf: hasOwnText(element) && element.children.length === 0,
    },
  };
}
let selected = null;
// A plain click selects one node. A click with the command key (or
// control) adds the node to the selection, or removes it again. The
// last node clicked is the primary one: the inspector shows it, and
// Pin to chat in the context menu names every node in the selection.
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
  // A nudge burst belongs to one node; a new selection ends it.
  isNudgeActive = false;
  updateHandles();
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
  // One snapshot per text-edit session; the browser undoes keystrokes.
  recordSnapshot(rootOf(element));
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
    if (item.isDanger) { button.setAttribute('data-swift-design-danger', ''); }
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
    case 'width': element.style.width = value; break;
    case 'height': element.style.height = value; break;
    case 'reset-size': element.style.removeProperty('width'); element.style.removeProperty('height'); break;
    case 'rotate': element.style.rotate = value; break;
    case 'reset-rotation': element.style.removeProperty('rotate'); break;
    case 'reset-position': element.style.removeProperty('translate'); break;
    case 'select_parent': if (element.parentElement && element.parentElement !== root) { select(element.parentElement); } else { select(root); } return false;
    default: return false;
  }
  return true;
}
const DRAG_THRESHOLD = 4;
const RESIZE_MINIMUM = 16;
const SNAP_THRESHOLD = 6;
const SNAP_SIBLING_LIMIT = 24;
let drag = null;
let resize = null;
let rotate = null;
let isClickSuppressed = false;
// The snap targets for one gesture: the screen-pixel positions of the
// root's edges and centers plus the siblings' edges and centers.
// Cached at the first move, because a translate never reflows them.
function snapLinesOf(root, element) {
  const vertical = [];
  const horizontal = [];
  const add = (rect) => {
    vertical.push(rect.left, rect.left + rect.width / 2, rect.right);
    horizontal.push(rect.top, rect.top + rect.height / 2, rect.bottom);
  };
  add(root.getBoundingClientRect());
  const siblings = element.parentElement ? Array.from(element.parentElement.children) : [];
  siblings.filter((node) => node !== element).slice(0, SNAP_SIBLING_LIMIT).forEach((node) => add(node.getBoundingClientRect()));
  return { vertical, horizontal };
}
// The nearest line within the threshold of any moving edge, as the
// screen-pixel correction to add to the pointer delta.
function snapAdjust(lines, edges) {
  let best = null;
  edges.forEach((edge) => {
    lines.forEach((line) => {
      const distance = line - edge;
      if (Math.abs(distance) <= SNAP_THRESHOLD && (!best || Math.abs(distance) < Math.abs(best.distance))) {
        best = { distance, line };
      }
    });
  });
  return best;
}
// Positions the bounding box with the handles over the primary
// selected node, or hides it. The overlay is fixed and the rect is
// viewport-relative, so the two always agree, whatever scrolls. A
// rotated node reports its axis-aligned rect, so the frame grows
// around it and resize still edits the unrotated width and height.
function updateHandles() {
  if (!selected || !selected.isConnected || !rootOf(selected) || selected === rootOf(selected)) {
    handleBox.style.display = 'none';
    return;
  }
  const rect = selected.getBoundingClientRect();
  handleBox.style.left = rect.left + 'px';
  handleBox.style.top = rect.top + 'px';
  handleBox.style.width = rect.width + 'px';
  handleBox.style.height = rect.height + 'px';
  handleBox.style.display = 'block';
}
// The layout size of the element in canvas pixels. The root scales
// through a transform, which never changes layout, so layout pixels
// are canvas pixels. The base stylesheet forces border-box; an inline
// override or an SVG element falls back to the computed style.
function baseSizeOf(element) {
  const style = getComputedStyle(element);
  if (element.offsetWidth === undefined || style.boxSizing !== 'border-box') {
    return { width: parseFloat(style.width) || 0, height: parseFloat(style.height) || 0 };
  }
  return { width: element.offsetWidth, height: element.offsetHeight };
}
// Without capture the document stops getting moves the moment the
// pointer crosses the preview's edge, and the gesture goes dead until
// the pointer comes back. Capture keeps the moves coming to the page.
function capturePointer(event) {
  if (!event.target.setPointerCapture) { return; }
  try { event.target.setPointerCapture(event.pointerId); } catch (error) {}
}
function startResize(event, handle) {
  if (event.button !== 0 || !selected) { return; }
  event.preventDefault();
  event.stopPropagation();
  capturePointer(event);
  const root = rootOf(selected);
  const base = baseSizeOf(selected);
  const rect = selected.getBoundingClientRect();
  // Per-axis scales from the element itself absorb the root's two
  // stacked scales and any transform an ancestor carries.
  const fallback = root.getBoundingClientRect().width / canvasWidth || 1;
  resize = {
    element: selected, root, handle, moved: false,
    x: event.clientX, y: event.clientY,
    baseWidth: base.width, baseHeight: base.height,
    baseTranslate: translateOf(selected),
    scaleX: base.width ? rect.width / base.width : fallback,
    scaleY: base.height ? rect.height / base.height : fallback,
    ratio: base.height ? base.width / base.height : 1,
    snapLines: snapLinesOf(root, selected),
    startRect: rect,
  };
}
// The active edge snaps in screen pixels before the scale division,
// like a move; the correction returns as an adjusted pointer delta.
function snappedResizeDeltas(event) {
  const handle = resize.handle;
  const rect = resize.startRect;
  let pointerDx = event.clientX - resize.x;
  let pointerDy = event.clientY - resize.y;
  let verticalSnap = null;
  let horizontalSnap = null;
  if (handle.includes('e')) { verticalSnap = snapAdjust(resize.snapLines.vertical, [rect.right + pointerDx]); }
  if (handle.includes('w')) { verticalSnap = snapAdjust(resize.snapLines.vertical, [rect.left + pointerDx]); }
  if (handle.includes('s')) { horizontalSnap = snapAdjust(resize.snapLines.horizontal, [rect.bottom + pointerDy]); }
  if (handle.includes('n')) { horizontalSnap = snapAdjust(resize.snapLines.horizontal, [rect.top + pointerDy]); }
  if (verticalSnap) { pointerDx += verticalSnap.distance; }
  if (horizontalSnap) { pointerDy += horizontalSnap.distance; }
  setGuide('x', verticalSnap ? verticalSnap.line : null);
  setGuide('y', horizontalSnap ? horizontalSnap.line : null);
  return { dx: pointerDx / resize.scaleX, dy: pointerDy / resize.scaleY };
}
function applyResize(event) {
  event.preventDefault();
  if (!resize.moved) { recordSnapshot(resize.root); }
  resize.moved = true;
  hideMenu();
  const handle = resize.handle;
  const deltas = snappedResizeDeltas(event);
  const dx = deltas.dx;
  const dy = deltas.dy;
  let width = resize.baseWidth;
  let height = resize.baseHeight;
  let shiftX = 0;
  let shiftY = 0;
  if (handle.includes('e')) { width += dx; }
  if (handle.includes('w')) { width -= dx; shiftX = dx; }
  if (handle.includes('s')) { height += dy; }
  if (handle.includes('n')) { height -= dy; shiftY = dy; }
  if (event.shiftKey && handle.length === 2) { height = width / resize.ratio; }
  // The clamp moves the translate shift in lockstep, so the anchored
  // edge stays put when the pointer overshoots the minimum.
  if (width < RESIZE_MINIMUM) {
    if (shiftX) { shiftX -= RESIZE_MINIMUM - width; }
    width = RESIZE_MINIMUM;
  }
  if (height < RESIZE_MINIMUM) {
    if (shiftY) { shiftY -= RESIZE_MINIMUM - height; }
    height = RESIZE_MINIMUM;
  }
  if (handle.includes('e') || handle.includes('w')) { resize.element.style.width = Math.round(width) + 'px'; }
  if (handle.includes('n') || handle.includes('s') || (event.shiftKey && handle.length === 2)) {
    resize.element.style.height = Math.round(height) + 'px';
  }
  if (shiftX || shiftY) {
    const x = Math.round(resize.baseTranslate.x + shiftX);
    const y = Math.round(resize.baseTranslate.y + shiftY);
    resize.element.style.translate = x + 'px ' + y + 'px';
  }
  correctResizeAnchor();
  updateHandles();
}
// Layout decides where a new width goes: a centered or right-anchored
// node grows to the left when only the east handle moved. The
// correction measures where the edge opposite the handle actually
// landed and moves the difference into `translate`, so that edge
// stays put whatever the layout does. A rotated node resizes around
// its center, so it is left alone.
function correctResizeAnchor() {
  if (rotationOf(resize.element)) { return; }
  const rect = resize.element.getBoundingClientRect();
  const start = resize.startRect;
  const handle = resize.handle;
  let dx = 0;
  let dy = 0;
  if (handle.includes('e')) { dx = start.left - rect.left; }
  if (handle.includes('w')) { dx = start.right - rect.right; }
  if (handle.includes('s')) { dy = start.top - rect.top; }
  if (handle.includes('n')) { dy = start.bottom - rect.bottom; }
  if (Math.abs(dx) < 0.5 && Math.abs(dy) < 0.5) { return; }
  const base = translateOf(resize.element);
  const x = Math.round(base.x + dx / resize.scaleX);
  const y = Math.round(base.y + dy / resize.scaleY);
  resize.element.style.translate = x + 'px ' + y + 'px';
}
function endResize(save) {
  if (!resize) { return; }
  const finished = resize;
  resize = null;
  hideGuides();
  if (!finished.moved) { return; }
  isClickSuppressed = true;
  if (save) { postHtml(finished.root, true); }
}
// Rotation lives in the standalone `rotate` property, like the drag
// offset in `translate`, so it never fights a screen's own transform.
const ROTATE_SNAP_DEGREES = 15;
const ROTATE_ZERO_DEGREES = 3;
function rotationOf(element) {
  const match = /(-?[\d.]+)deg/.exec(element.style.rotate || '');
  return match ? parseFloat(match[1]) : 0;
}
function pointerAngleOf(event, center) {
  return Math.atan2(event.clientY - center.y, event.clientX - center.x) * 180 / Math.PI;
}
function startRotate(event) {
  if (event.button !== 0 || !selected) { return; }
  event.preventDefault();
  event.stopPropagation();
  capturePointer(event);
  const rect = selected.getBoundingClientRect();
  const center = { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
  rotate = {
    element: selected, root: rootOf(selected), moved: false,
    center, baseAngle: rotationOf(selected),
    startPointer: pointerAngleOf(event, center),
  };
}
function applyRotate(event) {
  event.preventDefault();
  if (!rotate.moved) { recordSnapshot(rotate.root); hideMenu(); }
  rotate.moved = true;
  let degrees = rotate.baseAngle + pointerAngleOf(event, rotate.center) - rotate.startPointer;
  if (event.shiftKey) { degrees = Math.round(degrees / ROTATE_SNAP_DEGREES) * ROTATE_SNAP_DEGREES; }
  degrees = ((degrees + 180) % 360 + 360) % 360 - 180;
  if (Math.abs(degrees) <= ROTATE_ZERO_DEGREES) {
    rotate.element.style.removeProperty('rotate');
  } else {
    rotate.element.style.rotate = Math.round(degrees) + 'deg';
  }
  updateHandles();
}
function endRotate(save) {
  if (!rotate) { return; }
  const finished = rotate;
  rotate = null;
  if (!finished.moved) { return; }
  isClickSuppressed = true;
  if (save) { postHtml(finished.root, true); }
}
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
  hideGuides();
  if (!finished.moved) { return; }
  finished.root.removeAttribute('data-swift-design-dragging');
  isClickSuppressed = true;
  if (save) { postHtml(finished.root, true); }
}
document.addEventListener('pointermove', (event) => {
  if (rotate) { applyRotate(event); return; }
  if (resize) { applyResize(event); return; }
  if (!drag) { return; }
  const dx = event.clientX - drag.x;
  const dy = event.clientY - drag.y;
  if (!drag.moved) {
    if (Math.abs(dx) < DRAG_THRESHOLD && Math.abs(dy) < DRAG_THRESHOLD) { return; }
    drag.moved = true;
    recordSnapshot(drag.root);
    drag.snapLines = snapLinesOf(drag.root, drag.element);
    drag.startRect = drag.element.getBoundingClientRect();
    drag.root.setAttribute('data-swift-design-dragging', '');
    hideMenu();
    select(drag.element);
  }
  event.preventDefault();
  // The snap correction is applied to the screen-pixel delta, before
  // the scale division, so the snapped edge lands exactly on the line.
  let snappedDx = dx;
  let snappedDy = dy;
  const rect = drag.startRect;
  const verticalSnap = snapAdjust(drag.snapLines.vertical, [rect.left + dx, rect.left + rect.width / 2 + dx, rect.right + dx]);
  const horizontalSnap = snapAdjust(drag.snapLines.horizontal, [rect.top + dy, rect.top + rect.height / 2 + dy, rect.bottom + dy]);
  if (verticalSnap) { snappedDx += verticalSnap.distance; }
  if (horizontalSnap) { snappedDy += horizontalSnap.distance; }
  setGuide('x', verticalSnap ? verticalSnap.line : null);
  setGuide('y', horizontalSnap ? horizontalSnap.line : null);
  // Screen pixels divided by the root's scale give canvas pixels.
  const x = Math.round(drag.base.x + snappedDx / drag.scale);
  const y = Math.round(drag.base.y + snappedDy / drag.scale);
  drag.element.style.translate = x + 'px ' + y + 'px';
  updateHandles();
}, { passive: false });
document.addEventListener('pointerup', () => { endRotate(true); endResize(true); endDrag(true); });
document.addEventListener('pointercancel', () => { endRotate(false); endResize(false); endDrag(false); });
window.addEventListener('blur', () => { endRotate(true); endResize(true); endDrag(true); });
// Scroll does not bubble, so the reposition listens in capture.
document.addEventListener('scroll', updateHandles, true);
window.addEventListener('resize', updateHandles);
// An arrow key nudges the selected node by one canvas pixel, ten with
// Shift. With nothing selected the arrows still turn the page.
const NUDGE_STEP = 1;
const NUDGE_SHIFT_STEP = 10;
const NUDGE_SAVE_DELAY_MS = 400;
let nudgeTimer = null;
let isNudgeActive = false;
document.addEventListener('keydown', (event) => {
  if (!selected || !selected.isConnected || selected === rootOf(selected)) { return; }
  if (event.target && event.target.isContentEditable) { return; }
  const deltas = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
  const delta = deltas[event.key];
  if (!delta) { return; }
  event.preventDefault();
  event.stopImmediatePropagation();
  const root = rootOf(selected);
  // One undo snapshot and one save per burst of keys.
  if (!isNudgeActive) { recordSnapshot(root); isNudgeActive = true; }
  const step = event.shiftKey ? NUDGE_SHIFT_STEP : NUDGE_STEP;
  const base = translateOf(selected);
  selected.style.translate = (base.x + delta[0] * step) + 'px ' + (base.y + delta[1] * step) + 'px';
  updateHandles();
  clearTimeout(nudgeTimer);
  nudgeTimer = setTimeout(() => { isNudgeActive = false; postHtml(root, true); }, NUDGE_SAVE_DELAY_MS);
}, true);
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
        { label: 'Background color', color: true, value: toHex(getComputedStyle(root).backgroundColor) || '#ffffff', run: (value) => { recordCoalesced(root, 'menu-color'); applyAction(root, root, 'background', value); }, done: () => postHtml(root, true) },
        '-',
        { label: 'Add text', run: () => { recordSnapshot(root); applyAction(root, root, 'add-text'); postHtml(root, true); } },
        { label: 'Add image', run: () => { recordSnapshot(root); applyAction(root, root, 'add-image'); postHtml(root, true); } },
        '-',
        { label: 'Ask AI about this screen', run: () => post({ type: 'swift-design-action', screen, action: 'ask', path: null }) },
        { label: 'Properties…', run: () => post({ type: 'swift-design-action', screen, action: 'properties', path: null }) },
        '-',
        { label: 'Delete screen', isDanger: true, run: () => post({ type: 'swift-design-action', screen, action: 'delete-screen', path: null }) },
      ]);
      return;
    }
    const element = editableTarget(event.target);
    // A right-click on a node already in the selection keeps the
    // selection, so Pin to chat can pin every node in it.
    if (selection.indexOf(element) < 0) { select(element); }
    const act = (action, value) => () => { recordSnapshot(root); if (applyAction(root, element, action, value)) { postHtml(root, true); } };
    const items = [];
    if (hasOwnText(element)) {
      items.push({ label: 'Bigger', run: act('bigger') });
      items.push({ label: 'Smaller', run: act('smaller') });
      items.push({ label: 'Align left', run: act('align', 'left') });
      items.push({ label: 'Align center', run: act('align', 'center') });
      items.push({ label: 'Align right', run: act('align', 'right') });
      items.push('-');
    }
    items.push({ label: 'Color', color: true, value: toHex(getComputedStyle(element).color) || '#000000', run: (value) => { recordCoalesced(root, 'menu-color'); applyAction(root, element, 'color', value); }, done: () => postHtml(root, true) });
    items.push('-');
    if (element.style.translate) { items.push({ label: 'Reset position', run: act('reset-position') }); }
    if (element.style.width || element.style.height) { items.push({ label: 'Reset size', run: act('reset-size') }); }
    if (element.style.rotate) { items.push({ label: 'Reset rotation', run: act('reset-rotation') }); }
    items.push({ label: 'Move forward', run: act('forward') });
    items.push({ label: 'Move backward', run: act('backward') });
    items.push({ label: 'Duplicate', run: act('duplicate') });
    items.push('-');
    items.push({ label: 'Pin to chat', run: () => post(Object.assign({ type: 'swift-design-action', screen, action: 'pin', selection: selection.map(brief) }, describe(element))) });
    items.push({ label: 'Properties…', run: () => post(Object.assign({ type: 'swift-design-action', screen, action: 'properties' }, describe(element))) });
    items.push('-');
    items.push({ label: 'Delete', isDanger: true, run: act('delete') });
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
  recordCoalesced(root, data.property + ':' + data.path);
  if (applyAction(root, element, data.property, data.value)) { postHtml(root, false); }
});
"##;
