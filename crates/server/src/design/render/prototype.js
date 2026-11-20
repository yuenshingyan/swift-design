(() => {
  const audits = [];
  let probing = false;
  for (const root of document.querySelectorAll('[data-swift-design-root]')) {
    const groups = new Map();
    const split = (value) => {
      const separator = value.indexOf(':');
      return [value.slice(0, separator), value.slice(separator + 1)];
    };
    for (const panel of root.querySelectorAll('[data-demo-panel]')) {
      const [key, state] = split(panel.dataset.demoPanel);
      if (!groups.has(key)) groups.set(key, { panels: [], actions: [], current: 0 });
      groups.get(key).panels.push({ state, panel });
    }
    const destination = (group, action) => {
      switch (action) {
        case '@previous': return group.current - 1;
        case '@next': return group.current + 1;
        case '@first': return 0;
        case '@last': return group.panels.length - 1;
        default: return group.panels.findIndex(({ state }) => state === action);
      }
    };
    const update = (group) => {
      group.panels.forEach(({ panel }, index) => {
        panel.hidden = index !== group.current;
        panel.inert = panel.hidden;
      });
      group.actions.forEach(({ anchor, action }) => {
        const target = destination(group, action);
        const relative = action.startsWith('@');
        anchor.toggleAttribute('data-swift-design-action-ready', true);
        if (relative) {
          anchor.removeAttribute('aria-current');
          anchor.setAttribute('aria-disabled', String(target < 0 || target >= group.panels.length || target === group.current));
        } else {
          anchor.removeAttribute('aria-disabled');
          if (target === group.current) anchor.setAttribute('aria-current', 'true');
          else anchor.removeAttribute('aria-current');
        }
      });
    };
    const move = (group, action) => {
      const target = destination(group, action);
      if (target < 0 || target >= group.panels.length || target === group.current) return;
      const moveFocus = group.panels[group.current].panel.contains(document.activeElement);
      group.current = target;
      update(group);
      if (moveFocus) {
        const panel = group.panels[target].panel;
        if (!panel.hasAttribute('tabindex')) panel.tabIndex = -1;
        panel.focus({ preventScroll: true });
      }
      if (!probing && window.swiftDesignFit) window.swiftDesignFit();
    };
    for (const anchor of root.querySelectorAll('a[href="#demo-panel"][data-demo-action]')) {
      const [key, action] = split(anchor.dataset.demoAction);
      const group = groups.get(key);
      if (!group || group.panels.length < 2 ||
          (!['@previous', '@next', '@first', '@last'].includes(action) && destination(group, action) < 0)) continue;
      group.actions.push({ anchor, action });
      anchor.addEventListener('click', (event) => {
        if (event.defaultPrevented) return;
        event.preventDefault();
        move(group, action);
      });
      anchor.addEventListener('keydown', (event) => {
        if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
        const keyAction = { ' ': action, ArrowLeft: '@previous', ArrowRight: '@next', Home: '@first', End: '@last' }[event.key];
        if (typeof keyAction !== 'string') return;
        event.preventDefault();
        move(group, keyAction);
      });
    }
    for (const [name, group] of groups) {
      if (group.actions.length === 0) continue;
      group.current = Math.max(0, group.panels.findIndex(({ panel }) => !panel.hidden));
      update(group);
      audits.push({ name, group, destination, update, root });
    }
  }
  if (!document.querySelector('[data-swift-design-prototype-audit]')) return;
  window.swiftDesignCheckPrototypes = () => {
    const active = document.activeElement;
    probing = true;
    const reports = [];
    const openedDisclosures = [];
    const restoreDisclosures = () => {
      for (const { disclosure, name } of openedDisclosures.splice(0)) {
        disclosure.open = false;
        if (name !== null) disclosure.setAttribute('name', name);
      }
    };
    try {
      for (const { name, group, destination, update, root } of audits) {
        const screen = Number(root.closest('[data-swift-design-screen]').dataset.swiftDesignScreen);
        const initial = group.current;
        const last = group.panels.length - 1;
        const report = { screen, group: name, clicks: 0, keys: 0, focus: 0, failures: 0, issues: [] };
        const fail = (issue) => {
          report.failures++;
          if (!report.issues.includes(issue) && report.issues.length < 4) report.issues.push(issue);
        };
        const check = (target, cause) => {
          if (group.current !== target || group.panels.some(({ panel }, i) => panel.hidden !== (i !== target) || panel.inert !== panel.hidden)) fail(`${cause}: wrong visible or interactive panel`);
          if (group.actions.some(({ anchor, action }) => {
            const next = destination(group, action);
            return action.startsWith('@')
              ? anchor.hasAttribute('aria-current') || anchor.getAttribute('aria-disabled') !== String(next < 0 || next > last || next === target)
              : anchor.hasAttribute('aria-disabled') || (anchor.getAttribute('aria-current') === 'true') !== (next === target);
          })) fail(`${cause}: selection or disabled state disagrees with ARIA`);
        };
        for (const { anchor, action } of group.actions) {
          const panel = anchor.closest('[data-demo-panel]');
          const index = group.panels.findIndex((entry) => entry.panel === panel);
          group.current = index < 0 ? initial : index;
          update(group);
          const target = destination(group, action);
          const expected = target >= 0 && target <= last ? target : group.current;
          // Closed details may give their content layout rectangles even though it cannot receive focus.
          for (let ancestor = anchor.parentElement; ancestor && ancestor !== root; ancestor = ancestor.parentElement) {
            if (!(ancestor instanceof HTMLDetailsElement) || ancestor.open ||
                ancestor.querySelector(':scope > summary')?.contains(anchor)) continue;
            const name = ancestor.getAttribute('name');
            // Opening a named disclosure must not close an already-open peer during the probe.
            if (name !== null) ancestor.removeAttribute('name');
            openedDisclosures.push({ disclosure: ancestor, name });
            ancestor.open = true;
          }
          if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
          const visible = anchor.getClientRects().length && !anchor.closest('[inert], [hidden]') && getComputedStyle(anchor).visibility === 'visible';
          if (visible) {
            anchor.focus({ preventScroll: true });
            if (document.activeElement !== anchor || anchor.tabIndex < 0) fail(`${action}: control is not keyboard-focusable`);
            report.focus++;
          }
          const focusMoves = visible && index >= 0 && index !== expected;
          const beforeTab = group.panels[expected].panel.getAttribute('tabindex');
          anchor.click();
          report.clicks++;
          check(expected, `click ${action}`);
          if (focusMoves && document.activeElement !== group.panels[expected].panel) fail(`${action}: focus did not move out of the hidden panel`);
          if (beforeTab === null) group.panels[expected].panel.removeAttribute('tabindex');
          group.current = index < 0 ? initial : index;
          update(group);
          const event = new KeyboardEvent('keydown', { key: ' ', bubbles: true, cancelable: true });
          anchor.dispatchEvent(event);
          if (!event.defaultPrevented) fail(`Space on ${action}: key was not handled`);
          report.keys++;
          check(expected, `Space on ${action}`);
          restoreDisclosures();
        }
        const anchor = group.actions[0].anchor;
        for (const [key, from, expected] of [['Home', last, 0], ['End', 0, last], ['ArrowRight', 0, 1], ['ArrowLeft', last, last - 1], ['ArrowLeft', 0, 0], ['ArrowRight', last, last]]) {
          group.current = from;
          update(group);
          const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
          anchor.dispatchEvent(event);
          if (!event.defaultPrevented) fail(`${key}: key was not handled`);
          report.keys++;
          check(expected, key);
        }
        group.current = initial;
        update(group);
        reports.push(report);
      }
      return reports;
    } finally {
      restoreDisclosures();
      probing = false;
      if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
      if (active instanceof HTMLElement) active.focus({ preventScroll: true });
    }
  };
})();
