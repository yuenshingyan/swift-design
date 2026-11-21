const surface = document.querySelector('.annotation-surface');
if (!surface) return;
const outline = surface.querySelector('.annotation-drag');
let drag;
const point = (event) => {
  const { left, top, width, height } = surface.getBoundingClientRect();
  return [
    Math.max(0, Math.min(1, (event.clientX - left) / width)),
    Math.max(0, Math.min(1, (event.clientY - top) / height)),
  ];
};
const clear = () => {
  drag = undefined;
  outline.setAttribute('visibility', 'hidden');
};
surface.addEventListener('pointerdown', (event) => {
  if (!event.isPrimary || event.button !== 0 || drag) return;
  surface.focus({ preventScroll: true });
  surface.setPointerCapture(event.pointerId);
  drag = { id: event.pointerId, start: point(event), px: event.clientX, py: event.clientY };
  event.preventDefault();
});
surface.addEventListener('pointermove', (event) => {
  if (drag?.id !== event.pointerId) return;
  const end = point(event);
  const [x, y] = drag.start;
  outline.setAttribute('x', Math.min(x, end[0]));
  outline.setAttribute('y', Math.min(y, end[1]));
  outline.setAttribute('width', Math.abs(x - end[0]));
  outline.setAttribute('height', Math.abs(y - end[1]));
  outline.setAttribute('visibility', 'visible');
});
surface.addEventListener('pointerup', (event) => {
  if (drag?.id !== event.pointerId) return;
  if (Math.abs(event.clientX - drag.px) >= 4 && Math.abs(event.clientY - drag.py) >= 4) {
    dioxus.send([...drag.start, ...point(event)]);
  }
  clear();
});
surface.addEventListener('lostpointercapture', clear);
surface.addEventListener('pointercancel', clear);
surface.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && drag) {
    surface.releasePointerCapture(drag.id);
    clear();
  }
});
