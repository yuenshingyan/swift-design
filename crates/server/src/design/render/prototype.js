(() => {
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
          anchor.setAttribute('aria-disabled', String(target < 0 || target >= group.panels.length || target === group.current));
        } else if (target === group.current) {
          anchor.setAttribute('aria-current', 'true');
        } else {
          anchor.removeAttribute('aria-current');
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
      if (window.swiftDesignFit) window.swiftDesignFit();
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
    for (const group of groups.values()) {
      if (group.actions.length === 0) continue;
      group.current = Math.max(0, group.panels.findIndex(({ panel }) => !panel.hidden));
      update(group);
    }
  }
})();
