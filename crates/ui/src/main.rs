//! Swift Design editor: the browser UI for tweaking designs.
//!
//! Compiled to WASM with `dx build --release` and served by the
//! `server` crate. Run in development with `dx serve` from this
//! crate's directory.

mod api;
mod artwork_editor;
mod campaign_editor;
mod canvas;
mod chat;
mod chat_controls;
mod deck_editor;
mod document_editor;
mod editor;
mod home;
mod icons;
mod mailing_editor;
mod print_editor;
mod prompt_history;
mod question_card;
mod revert;
mod route;
mod run_settings;
mod select;
mod session;
mod settings;
mod social_editor;
mod status;
mod uploads;

use dioxus::document;
use dioxus::prelude::*;

use crate::route::View;

/// Embedded stylesheet: the editor ships as one WASM bundle with no
/// separate asset pipeline yet. The palette and type follow the
/// product canvas: three paper surfaces, ink text, one teal accent that
/// means "working" or "selected", Inter for text, JetBrains Mono for
/// identifiers. Radii, shadows, and colours are tokens on `:root`.
const STYLESHEET: &str = "
:root {
  --paper: #F7F6F3; --raised: #FFFFFF; --sunken: #F1EFEA; --subtle: #FBFAF8;
  --ink: #15181C; --ink-2: #4E545B; --muted: #6C7178; --faint: #9A9EA4; --ghost: #A5A29A;
  --hairline: #E7E5DF; --hairline-2: #EEECE6; --line: #D6D3CB; --line-soft: #DCD9D2;
  --teal: #0E6E63; --teal-hover: #0B564D; --teal-tint: #EAF3F1; --teal-line: #B9D6D0;
  --danger: #B4231F; --error: #A34A2B;
  --r-badge: 4px; --r-control: 6px; --r-button: 7px; --r-primary: 8px;
  --r-panel: 10px; --r-card: 12px; --r-shell: 14px;
  /* The composer controls: one 34px track, one radius, in every chat. */
  --track: 2.125rem; --r-track: 10px;
  --sh-control: 0 1px 1px rgba(21,24,28,.03);
  --sh-card: 0 1px 2px rgba(21,24,28,.04), 0 18px 40px -34px rgba(21,24,28,.5);
  --sh-float: 0 20px 44px -34px rgba(21,24,28,.5), 0 1px 2px rgba(21,24,28,.04);
  --sh-primary: 0 1px 2px rgba(21,24,28,.28), inset 0 1px 0 rgba(255,255,255,.09);
  --mono: 'JetBrains Mono', ui-monospace, monospace;
}
*:focus-visible { outline: 2px solid var(--teal); outline-offset: 2px; }
@media (prefers-reduced-motion: reduce) {
  .status-dot { animation: none; }
  * { transition-duration: 1ms !important; }
}

body { margin: 0; background: var(--paper); color: var(--ink);
  font-family: Inter, system-ui, sans-serif; overflow-x: hidden; }
code, .mono { font-family: var(--mono); }
.topbar { display: flex; align-items: center; justify-content: space-between;
  padding: 1rem 1.75rem; border-bottom: 1px solid #E3E1DB; background: var(--raised); }
.brand { display: flex; align-items: center; gap: 0.6rem;
  font-size: 0.95rem; font-weight: 600; letter-spacing: -0.01em;
  border: 0; background: transparent; padding: 0; cursor: pointer; box-shadow: none; }
.brand:hover:not(:disabled) { background: transparent; }
.topbar-context { display: flex; align-items: center; gap: 0.75rem;
  font-family: var(--mono); font-size: 0.72rem; color: var(--muted); }
.topbar-context .status-dot { width: 6px; height: 6px; }
.topbar-context .ok { color: var(--teal); }
button, .button { font: inherit; font-size: 0.8rem; cursor: pointer; color: var(--ink);
  border: 1px solid var(--line); border-radius: var(--r-button); background: var(--raised);
  padding: 0.42rem 0.7rem; text-decoration: none; white-space: nowrap;
  box-shadow: var(--sh-control);
  transition: background-color 120ms ease, border-color 120ms ease, box-shadow 120ms ease,
    opacity 120ms ease; }
button:hover:not(:disabled), .button:hover { background: var(--subtle); border-color: #B4B0A7; }
button:disabled { opacity: .45; cursor: default; box-shadow: none; color: var(--ink); }
button.primary { background: var(--ink); border-color: var(--ink); color: var(--paper);
  font-size: 0.82rem; font-weight: 600; padding: 0.62rem 1.1rem;
  border-radius: var(--r-button); box-shadow: var(--sh-primary); }
button.primary:hover:not(:disabled) { background: #23272C; border-color: #23272C; }
button.primary:active:not(:disabled) { transform: translateY(0.5px); box-shadow: none; }
button.primary:disabled { box-shadow: none; }
.divider { width: 1px; height: 1.125rem; background: #E3E1DB; flex: none; }
.icon-button { width: 1.625rem; height: 1.625rem; padding: 0; flex: none;
  display: inline-flex; align-items: center; justify-content: center;
  border-radius: var(--r-control); color: var(--muted); font-size: 0.9rem; line-height: 1; }
.icon-button span { display: flex; }
.icon-button:hover:not(:disabled) { color: var(--ink); }
.kicker { font-family: var(--mono);
  font-size: 0.7rem; letter-spacing: 0.1em; text-transform: uppercase;
  color: var(--teal); }
.error, .message { margin: 0.25rem 0; font-size: 0.85rem; }
.error { color: var(--error); }
.message { color: var(--teal); }
.lede { margin: 0; font-size: 0.875rem; color: var(--muted); }

/* Home: prompt box on top, projects below */
.home { display: flex; flex-direction: column; align-items: center;
  gap: 2.25rem; padding: 4.5rem 2rem 4rem; box-sizing: border-box;
  min-height: calc(100vh - 3.6rem); overflow-y: auto; }
.home-hero { display: flex; flex-direction: column; align-items: center; gap: 0.6rem; }
.home-hero h1 { margin: 0; font-size: 2.625rem; line-height: 1.05; letter-spacing: -0.04em;
  font-weight: 600; text-align: center; }
.home-composer { width: 100%; max-width: 46rem; display: flex;
  flex-direction: column; gap: 1rem; position: relative; }
.home-controls { display: flex; align-items: center; gap: 0.5rem;
  flex-wrap: nowrap; white-space: nowrap; min-width: 0; }
.control-group { display: flex; align-items: stretch; flex: none; overflow: hidden;
  border: 1px solid var(--line); border-radius: var(--r-button);
  background: var(--raised); box-shadow: var(--sh-control); }
.control-group > * + * { border-left: 1px solid var(--hairline-2); }
.control-cell { display: flex; align-items: center; gap: 0.375rem;
  padding: 0 0.25rem 0 0.5625rem; white-space: nowrap; cursor: pointer; }
.control-label { font-size: 0.75rem; color: #8A8F96; }
.control-value { font-size: 0.75rem; font-weight: 500; color: var(--ink); }
button.control-cell { border: 0; border-radius: 0; background: transparent; box-shadow: none;
  padding: 0.375rem 0.5625rem; }
button.control-cell:hover:not(:disabled) { background: var(--subtle); border-color: transparent; }
button.control-cell:focus-visible { outline-offset: -2px; }
.control-group .select-trigger { font-size: 0.75rem; font-weight: 500; color: var(--ink);
  border: 0; border-radius: 0; background: transparent; box-shadow: none; width: auto;
  padding: 0.375rem 0.3125rem; gap: 0.3rem; }
.control-group .select-trigger:hover:not(:disabled) { background: transparent; }
.control-group .select-trigger:focus-visible { outline-offset: -2px; }
.control-group .select-menu { min-width: 9rem; }
.template-button { display: inline-flex; align-items: center; justify-content: center;
  gap: 0.3rem; flex: none; height: var(--track); min-width: var(--track); padding: 0 0.5rem;
  border-radius: var(--r-track); color: var(--ink-2); }
.template-button span { display: flex; }
.template-button.chosen { border-color: var(--teal-line); background: var(--teal-tint);
  color: var(--teal); }
.template-button.chosen:hover:not(:disabled) { background: var(--teal-tint);
  border-color: var(--teal); }
.template-button-count { font-size: 0.72rem; font-weight: 600; line-height: 1; }

/* Dropdown picker */
.select { position: relative; min-width: 0; }
.select-trigger { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem;
  width: 100%; text-align: left; font-size: 0.84rem; color: var(--ink);
  padding: 0.5625rem 0.6875rem; border: 1px solid var(--line); border-radius: var(--r-button);
  background: var(--raised); box-shadow: inset 0 1px 1px rgba(21,24,28,.03); }
.select-trigger:hover:not(:disabled) { background: var(--raised); border-color: #B4B0A7; }
.select-value { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.select-chevron { display: flex; color: var(--faint); flex: none; }
.select-menu { position: fixed; z-index: 6; max-width: min(24rem, 90vw);
  max-height: 16rem; overflow-y: auto; display: flex; flex-direction: column; gap: 1px;
  background: var(--raised); border: 1px solid #E0DDD6; border-radius: var(--r-panel);
  box-shadow: 0 18px 40px -30px rgba(21,24,28,.5), 0 1px 2px rgba(21,24,28,.04);
  padding: 0.25rem; box-sizing: border-box; }
.select-option { display: flex; align-items: center; justify-content: space-between; gap: 0.75rem;
  width: 100%; text-align: left; border: 0; border-radius: var(--r-control);
  background: transparent; box-shadow: none; font-size: 0.8125rem; color: var(--ink);
  padding: 0.45rem 0.7rem; white-space: nowrap; }
.select-option:hover:not(:disabled) { background: var(--subtle); border-color: transparent; }
.select-option.selected { font-weight: 500; }
.select-option .tick { display: flex; color: var(--teal); }
.select-option:focus-visible { outline-offset: -2px; }
.model-chip { display: inline-flex; align-items: center; gap: 0.4rem; flex: none;
  min-width: 0; max-width: 13rem; height: var(--track); box-sizing: border-box;
  border: 1px solid var(--line); border-radius: var(--r-track);
  background: var(--raised); padding: 0 0.75rem 0 0.625rem; box-shadow: var(--sh-control);
  font-family: var(--mono); font-size: 0.74rem; color: var(--ink-2); }
.model-chip-text { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.model-chip-chevron { display: flex; color: var(--faint); flex: none; }
.model-chip-wrap { position: relative; min-width: 0; flex: none; display: flex; align-items: center; }
.model-chip-wrap .error { position: absolute; right: 0; bottom: 100%; margin: 0 0 0.25rem;
  font-size: 0.72rem; white-space: nowrap; }

/* Popover menus (model chip) */
.popover-menu { position: fixed; z-index: 6; min-width: 15rem; max-width: min(22rem, 90vw);
  max-height: 22rem; overflow-y: auto; display: flex; flex-direction: column; gap: 1px;
  background: var(--raised); border: 1px solid #E0DDD6; border-radius: var(--r-panel);
  box-shadow: 0 18px 40px -30px rgba(21,24,28,.5), 0 1px 2px rgba(21,24,28,.04);
  padding: 0.25rem; box-sizing: border-box; }
.menu-item { display: flex; align-items: center; gap: 0.5rem; width: 100%; text-align: left;
  border: 0; border-radius: var(--r-control); background: transparent; box-shadow: none;
  font-size: 0.8125rem; color: var(--ink); padding: 0.5rem 0.7rem; white-space: nowrap; }
.menu-item:hover:not(:disabled) { background: var(--subtle); border-color: transparent; }
.menu-item:focus-visible { outline-offset: -2px; }
.menu-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.menu-title.mono { font-size: 0.75rem; }
.menu-value { color: var(--faint); font-size: 0.78rem; }
.menu-glyph, .menu-tick { display: flex; flex: none; color: var(--faint); }
.menu-tick { color: var(--teal); }
.menu-rule { height: 1px; background: var(--hairline-2); margin: 0.25rem 0; }
.model-chip::before { content: ''; width: 6px; height: 6px; border-radius: 999px;
  background: var(--teal); flex: none; }
.model-chip.unset::before { background: var(--faint); }
/* The setup panel lies over the composer instead of pushing it down:
   nothing below it is usable until a model is chosen. */
.home-setup, .chat-settings { position: absolute; left: 0; right: 0; z-index: 7;
  min-height: 100%; box-sizing: border-box; background: var(--raised);
  border: 1px solid var(--line-soft); border-radius: var(--r-card);
  box-shadow: var(--sh-float); overflow: hidden; }
.home-setup { top: 0; }
.chat-settings { bottom: 0; max-height: 60vh; overflow-y: auto; }
.home-projects { width: 100%; max-width: 46rem; display: flex;
  flex-direction: column; }
.projects-head { display: flex; align-items: baseline; justify-content: space-between;
  padding: 0 0.75rem 0.6rem; }
.home-projects h2 { margin: 0; font-family: var(--mono); font-size: 0.7rem;
  letter-spacing: 0.11em; text-transform: uppercase; font-weight: 500; color: var(--muted); }
.projects-count { font-family: var(--mono); font-size: 0.7rem; color: var(--faint); }
.home-empty { margin: 0; font-size: 0.9rem; color: var(--muted); padding: 0 0.75rem; }
.project-rule { height: 1px; background: #E3E1DB; margin: 4px 0.75rem; }
.project-row { display: flex; align-items: center;
  justify-content: space-between; gap: 1rem; padding: 0.8rem 0.75rem;
  border: 1px solid transparent; border-radius: 9px;
  background: transparent; text-align: left; font-size: 0.9rem; cursor: pointer;
  transition: background-color 120ms ease, border-color 120ms ease, box-shadow 120ms ease; }
.project-row:hover, .project-row:focus-visible { background: var(--raised);
  border-color: var(--hairline); box-shadow: 0 1px 2px rgba(21,24,28,.04); }
.project-row .rename-input { flex: 1; }
.project-row .row-rename, .project-row .row-delete { flex: none; min-width: 1.5rem;
  width: 1.5rem; height: 1.5rem; padding: 0; border-radius: 999px;
  border: 1px solid #E3E1DB; background: var(--subtle); color: var(--muted);
  font-size: 0.8rem; line-height: 1; display: flex; align-items: center;
  justify-content: center; opacity: 0; box-shadow: none; }
.project-row .row-rename span { display: flex; }
.project-row:hover .row-rename, .project-row:hover .row-delete,
.project-row:focus-within .row-rename, .project-row:focus-within .row-delete,
.project-row .row-delete.confirm { opacity: 1; }
.project-row .row-rename:hover { border-color: var(--ink); color: var(--ink); }
.rename-input { font: inherit; font-size: 0.8rem; border: 1px solid var(--line);
  border-radius: var(--r-button); padding: 0.35rem 0.5rem; background: var(--raised);
  color: var(--ink); min-width: 0; }
.project-row .row-delete:hover { border-color: var(--danger); color: var(--danger); }
.project-row .row-delete.confirm { width: auto; padding: 0 0.6rem; border-color: var(--danger);
  background: var(--danger); color: #FFFFFF;
  font-family: var(--mono); font-size: 0.65rem; }
.project-title { flex: 1; font-size: 0.9375rem; font-weight: 600; letter-spacing: -0.01em;
  /* A long name is cut with an ellipsis. min-width lets a flex
     item shrink below its text width, which the cut needs. */
  min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.project-count { font-size: 0.72rem; color: var(--muted); white-space: nowrap; }
.project-time { font-family: var(--mono); font-size: 0.68rem; color: var(--faint);
  white-space: nowrap; }
.project-chevron { display: flex; color: var(--ink); opacity: .4; transition: opacity 120ms ease; }
.project-row:hover .project-chevron { opacity: 1; }

/* Studio: conversation beside the live canvas */
.studio { display: grid; grid-template-columns: 400px 1fr;
  align-items: stretch; height: calc(100vh - 3.6rem); }
.studio-head { display: flex; align-items: center; gap: 0.625rem;
  padding: 1rem 1.25rem 0.875rem; border-bottom: 1px solid #F0EEE9; }
.studio-head .back { display: inline-flex; align-items: center; gap: 0.25rem; border: 0;
  background: transparent; padding: 0.2rem 0; box-shadow: none;
  color: var(--muted); font-size: 0.8125rem; }
.studio-head .back span { display: flex; }
.studio-head .back:hover:not(:disabled) { background: transparent; color: var(--ink); }
.studio-head .divider { height: 0.875rem; }
.studio-head .kicker { font-size: 0.75rem; letter-spacing: 0; text-transform: none; }
.studio-head .rename { margin-left: auto; width: 1.375rem; height: 1.375rem; padding: 0;
  display: flex; align-items: center; justify-content: center; box-shadow: none;
  border: 1px solid var(--hairline); background: var(--subtle); border-radius: var(--r-control);
  color: var(--muted); }
.studio-head .rename span { display: flex; }
.studio-head .rename:hover:not(:disabled) { color: var(--ink); }
.studio-head .rename-input { flex: 1; min-width: 0; }
.conversation { display: flex; flex-direction: column; gap: 0.875rem;
  padding: 0 0 1.125rem; border-right: 1px solid var(--hairline);
  background: var(--raised); overflow: hidden; min-height: 0; }
.thread { display: flex; flex-direction: column; gap: 0.875rem; flex: 1;
  min-height: 0; overflow-y: auto; padding: 1.125rem 1.25rem 0; }
.conversation > .chat-box { margin: 0 1.25rem; }
.brief-summary { display: flex; flex-direction: column; gap: 0.625rem;
  border: 1px solid var(--hairline); border-radius: var(--r-panel); background: var(--subtle);
  padding: 0.8125rem 0.9375rem; }
.brief-summary-title { margin: 0; font-size: 0.875rem; font-weight: 600; line-height: 1.45;
  white-space: pre-wrap; }
.brief-tags { display: flex; flex-wrap: wrap; gap: 0.375rem; }
.brief-tags .badge { text-transform: none; letter-spacing: 0; font-size: 0.656rem;
  background: var(--raised); border-color: var(--hairline); }
.chat-box { border: 1px solid var(--line); border-radius: var(--r-card); background: var(--raised);
  display: flex; flex-direction: column; flex: none; position: relative;
  box-shadow: 0 1px 2px rgba(21,24,28,.04), 0 10px 24px -22px rgba(21,24,28,.4); }
.chat-box textarea { border: 0; outline: none; resize: none; min-height: 4.2rem;
  padding: 0.8rem 0.9rem; font: inherit; font-size: 0.9rem; line-height: 1.45;
  background: transparent; color: var(--ink); }
.chat-box textarea::placeholder { color: var(--ghost); }
.chat-box-row { display: flex; align-items: center; justify-content: space-between;
  gap: 0.5rem; padding: 0.4rem 0.625rem 0.625rem; min-width: 0; }
.chat-box-left, .chat-box-right { display: flex; align-items: center;
  gap: 0.45rem; flex-wrap: nowrap; min-width: 0; }
.chat-box-left { flex: 1; }
.chat-box-right { flex: none; max-width: 100%; }
.chat-box .brief-attachments { padding: 0 0.6rem 0.5rem; }
.chat-box .attach-button { width: var(--track); height: var(--track); }
.chat-note { font-size: 0.7rem; color: var(--muted); }
.revert-turn { align-self: flex-start; margin-top: -0.25rem; padding: 0.2rem 0.6rem;
  font-family: var(--mono); font-size: 0.68rem; border: 1px solid var(--line); border-radius: 999px;
  background: transparent; color: var(--ink-2); box-shadow: none; }
.revert-turn:hover:not(:disabled) { border-color: var(--teal); color: var(--teal); }
.revert-turn:disabled { opacity: 0.6; }
.chat-box-left .chat-note { overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  min-width: 0; }
button.primary.send-button { display: inline-flex; align-items: center; justify-content: center;
  flex: none; width: var(--track); height: var(--track); padding: 0;
  border-radius: var(--r-track); }
.send-button span { display: flex; }
.bubble { padding: 0.6875rem 0.875rem; font-size: 0.875rem; line-height: 1.5; max-width: 92%; }
.bubble p { margin: 0; white-space: pre-wrap; }
.bubble.user { align-self: flex-end; background: var(--ink); color: var(--paper);
  border-radius: 12px 12px 4px 12px; }
.bubble.agent { align-self: flex-start; background: var(--sunken); color: var(--ink);
  border-radius: 12px 12px 12px 4px; }
.status-card { border: 1px solid var(--hairline); border-radius: var(--r-panel);
  background: var(--raised); padding: 0.75rem 0.875rem; display: flex;
  flex-direction: column; gap: 0.5625rem; box-shadow: 0 1px 2px rgba(21,24,28,.03); }
.status-line { display: flex; align-items: center; gap: 0.6rem;
  font-size: 0.8125rem; color: var(--ink); font-weight: 500; }
.status-line .pct { margin-left: auto; font-family: var(--mono); font-size: 0.6875rem;
  color: var(--faint); font-weight: 400; }
.status-dot { width: 8px; height: 8px; border-radius: 50%; background: var(--teal); flex: none;
  animation: status-pulse 1.6s ease-in-out infinite; }
.status-dot.done { animation: none; }
.usage-line { margin: 0; font-family: var(--mono); font-size: 0.68rem; color: var(--faint); }
.status-card .usage-line { display: flex; align-items: center; gap: 0.75rem;
  border-top: 1px solid #F0EEE9; padding-top: 0.5625rem; }
.status-card .usage-line button { margin-left: auto; font-size: 0.75rem;
  padding: 0.25rem 0.6rem; border-radius: var(--r-control); }
.question-hint { margin: 0; font-size: 0.75rem; color: var(--muted); }
.agent-log { margin: 0; font-family: var(--mono); font-size: 0.68rem; color: var(--faint);
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 100%; }
/* A failure's log line is read whole, so it wraps instead of clipping. */
.agent-log.wrapped { white-space: normal; overflow-wrap: anywhere; }
.settings-head { display: flex; align-items: center; gap: 0.625rem;
  padding: 0.625rem 0.875rem; background: var(--subtle);
  border-bottom: 1px solid var(--hairline-2); }
.settings-head .kicker { font-size: 0.625rem; white-space: nowrap; }
.settings-head .icon-button { width: 1.375rem; height: 1.375rem; font-size: 0.8rem; }
.step-rail { margin-left: auto; display: flex; align-items: center; gap: 0.375rem; }
.step-rail .step { display: flex; align-items: center; gap: 0.3125rem; font-size: 0.6875rem;
  color: var(--faint); white-space: nowrap; }
.step-rail .step .n { width: 1rem; height: 1rem; border-radius: 999px;
  font-size: 0.5625rem; display: flex; align-items: center; justify-content: center;
  border: 1px solid var(--line-soft); background: var(--raised); }
.step-rail .step.current { color: var(--teal); }
.step-rail .step.current .n { background: var(--teal); border-color: var(--teal); color: #FFFFFF; }
.step-rail .step.done { color: var(--teal); }
.step-rail .step.done .n { background: var(--teal-tint); border-color: var(--teal-line);
  color: var(--teal); }
.step-rail .sep { width: 0.75rem; height: 1px; background: var(--line-soft); }
.settings-form { display: flex; flex-direction: column; gap: 0.75rem; padding: 0.875rem 1rem 1rem; }
.settings-form button { font-size: 0.75rem; padding: 0.4rem 0.7rem; }
.settings-form button.primary { font-size: 0.78rem; font-weight: 600; padding: 0.5rem 0.9rem; }
.settings-form .icon-button { width: 1.375rem; height: 1.375rem; }
.field-label { font-size: 0.75rem; color: var(--muted); }
.provider-name { margin: 0; font-size: 0.8rem; font-weight: 500; color: var(--muted);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.link-button { border: 0; background: transparent; box-shadow: none; padding: 0.2rem 0.3rem;
  color: var(--teal); font-size: 0.72rem; font-weight: 500; }
.link-button:hover:not(:disabled) { background: transparent; border-color: transparent;
  color: var(--teal-hover); text-decoration: underline; }
.field-heading .link-button { padding: 0 0.2rem; }
.settings-divider { display: flex; align-items: center; gap: 0.625rem;
  font-family: var(--mono); font-size: 0.65rem; letter-spacing: 0.04em; color: var(--faint);
  white-space: nowrap; }
.settings-divider::before, .settings-divider::after { content: ''; flex: 1; height: 1px;
  background: var(--hairline-2); }
.settings-form label, .settings-form .field { display: flex; flex-direction: column;
  gap: 0.375rem; font-size: 0.75rem; color: var(--muted); }
.settings-form .select-trigger { padding: 0.5rem 0.625rem; font-size: 0.8rem; }
.settings-form .provider-field .select { width: 100%; }
.settings-form input, .settings-form select { font: inherit; font-size: 0.8rem;
  color: var(--ink); border: 1px solid var(--line); border-radius: var(--r-button);
  padding: 0.5rem 0.625rem; background: var(--raised);
  box-shadow: inset 0 1px 1px rgba(21,24,28,.03); }
.settings-actions { display: flex; align-items: center; gap: 0.625rem; flex-wrap: wrap; }
.settings-login { display: flex; flex-direction: column; gap: 0.8rem;
  border-top: 1px solid var(--hairline); padding-top: 0.9rem; }
.settings-login .button { align-self: flex-start; }
.step-count { margin-left: auto; font-size: 0.7rem; color: var(--faint); white-space: nowrap; }
.key-field { position: relative; display: flex; }
.key-field input { flex: 1; min-width: 0; padding-right: 3.6rem; }
.key-field .link-button { position: absolute; right: 0.3rem; top: 50%; translate: 0 -50%; }
.key-status { display: flex; align-items: center; gap: 0.375rem; margin: 0;
  font-family: var(--mono); font-size: 0.68rem; color: var(--teal); }
.settings-form .lede { font-size: 0.72rem; line-height: 1.5; }
.model-count { display: flex; align-items: center; gap: 0.25rem;
  font-family: var(--mono); font-size: 0.68rem; color: var(--faint); }
.model-list { display: flex; flex-direction: column; overflow: hidden auto; max-height: 15rem;
  border: 1px solid var(--line); border-radius: var(--r-panel); background: var(--raised); }
.model-list .model-option { display: flex; align-items: flex-start; gap: 0.5rem; width: 100%;
  text-align: left; border: 0; border-radius: 0; background: transparent; box-shadow: none;
  padding: 0.5625rem 0.6875rem; font-size: 0.75rem; white-space: normal; }
.model-list .model-option + .model-option { border-top: 1px solid var(--hairline-2); }
.model-list .model-option:hover:not(:disabled) { background: var(--subtle); }
.model-list .model-option.selected, .model-list .model-option.selected:hover:not(:disabled) {
  background: var(--teal-tint); }
.model-list .model-option:disabled { opacity: .45; }
.model-list .model-option:focus-visible { outline-offset: -2px; }
.model-radio { width: 0.8125rem; height: 0.8125rem; flex: none; margin-top: 0.15rem;
  border: 1px solid var(--line); border-radius: 999px; background: var(--raised); }
.model-option.selected .model-radio { border-color: var(--teal); background: var(--teal);
  box-shadow: inset 0 0 0 2px var(--raised); }
.model-option-text { display: flex; flex-direction: column; gap: 0.15rem; min-width: 0; }
.model-id { font-family: var(--mono); font-size: 0.75rem; font-weight: 500; color: var(--ink);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.model-desc { font-size: 0.7rem; color: var(--muted); line-height: 1.4; }
.model-option .badge { margin-left: auto; flex: none; align-self: center; }

/* Modal shell */
.modal-backdrop { position: fixed; inset: 0; z-index: 20; background: rgba(21,24,28,.34); }
/* The kind picker blurs the page behind it: the request is sent and
   the one remaining choice is the modal. */
.modal-backdrop.blurring { background: rgba(21,24,28,.22);
  backdrop-filter: blur(6px); -webkit-backdrop-filter: blur(6px); }
.modal { position: fixed; z-index: 21; top: 50%; left: 50%; translate: -50% -50%;
  width: min(56rem, calc(100vw - 3rem)); max-height: calc(100vh - 4rem);
  display: flex; flex-direction: column; overflow: hidden;
  background: var(--raised); border: 1px solid var(--line-soft);
  border-radius: var(--r-shell); box-shadow: 0 32px 64px -28px rgba(21,24,28,.55),
    0 1px 2px rgba(21,24,28,.06); }
.modal-head { display: flex; align-items: center; gap: 0.625rem; flex: none;
  padding: 0.75rem 0.875rem 0.75rem 1rem; background: var(--subtle);
  border-bottom: 1px solid var(--hairline-2); }
.modal-note { font-family: var(--mono); font-size: 0.68rem; color: var(--faint); }
.modal-head .icon-button { margin-left: auto; }
.modal-body { padding: 1rem; overflow-y: auto; min-height: 0; }
.modal-foot { display: flex; align-items: center; gap: 0.625rem; flex: none;
  padding: 0.75rem 1rem; border-top: 1px solid var(--hairline-2); background: var(--subtle); }
.modal-foot .step-count { margin-left: 0; }
.modal-foot .primary { margin-left: auto; }

/* Template picker */
/* A screen renders at 1280x720 and is scaled to the card, so the design's
   own CSS sees the width it was written for. Card width and scale must
   stay in step: 224px / 1280px = 0.175. */
.template-grid { display: grid; gap: 0.875rem; justify-content: start;
  grid-template-columns: repeat(auto-fill, 14rem); }
.template-card { position: relative; border: 1px solid var(--line-soft);
  border-radius: var(--r-card); background: var(--raised); overflow: hidden;
  transition: border-color 120ms ease, box-shadow 120ms ease; }
.template-card:hover { border-color: #B4B0A7; box-shadow: 0 1px 2px rgba(21,24,28,.05); }
.template-card.chosen { border-color: var(--teal); box-shadow: 0 0 0 1px var(--teal); }
.template-card-hit { display: block; width: 100%; text-align: left; border: 0; border-radius: 0;
  background: transparent; box-shadow: none; padding: 0; white-space: normal; }
.template-card-hit:hover:not(:disabled) { background: transparent; }
.template-card-hit:focus-visible { outline-offset: -3px; }
.template-thumb { position: relative; width: 100%; aspect-ratio: 16 / 9; overflow: hidden;
  border-bottom: 1px solid var(--hairline); background: var(--sunken); }
.template-thumb iframe { position: absolute; top: 0; left: 0; width: 1280px; height: 720px;
  border: 0; pointer-events: none; transform: scale(0.175); transform-origin: top left; }
.template-meta { display: flex; flex-direction: column; gap: 0.15rem; padding: 0.5rem 0.625rem; }
.template-card-name { font-size: 0.8rem; font-weight: 600; letter-spacing: -0.01em;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.template-card-detail { font-family: var(--mono); font-size: 0.66rem; color: var(--faint);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.template-check { position: absolute; top: 0.5rem; right: 0.5rem; width: 1.125rem;
  height: 1.125rem; display: flex; align-items: center; justify-content: center;
  border: 1px solid var(--line); border-radius: var(--r-badge);
  background: rgba(255,255,255,.92); color: transparent; font-size: 0.7rem; line-height: 1; }
.template-card.chosen .template-check { border-color: var(--teal); background: var(--teal);
  color: #FFFFFF; }
.extract-form { display: grid; grid-template-columns: minmax(8rem, 1fr) minmax(10rem, 2fr) auto;
  gap: 0.5rem; align-items: center; margin-bottom: 1rem; padding: 0.75rem;
  border: 1px dashed var(--line); border-radius: var(--r-card); background: var(--subtle); }
.extract-title { grid-column: 1 / -1; font-family: var(--mono); font-size: 0.66rem;
  color: var(--muted); text-transform: uppercase; letter-spacing: 0.04em; }
.extract-form input { min-width: 0; padding: 0.4rem 0.6rem; border: 1px solid var(--line);
  border-radius: var(--r-button); background: var(--raised); font-size: 0.8rem; }
.extract-button { padding: 0.4rem 0.75rem; border: 1px solid var(--teal-line);
  border-radius: var(--r-button); background: var(--raised); color: var(--teal);
  font-family: var(--mono); font-size: 0.72rem; white-space: nowrap; }
.extract-button:disabled { border-color: var(--line); color: var(--muted); }
.template-card-default { position: absolute; right: 0.5rem; bottom: 0.5rem; height: 1.125rem;
  min-width: 1.125rem; padding: 0 0.3rem; display: flex; align-items: center;
  justify-content: center; border-radius: var(--r-badge); border: 1px solid var(--hairline);
  background: rgba(255,255,255,.92); color: var(--muted); font-family: var(--mono);
  font-size: 0.66rem; line-height: 1; box-shadow: none; opacity: 0; }
.template-card:hover .template-card-default, .template-card:focus-within .template-card-default,
.template-card-default.on { opacity: 1; }
.template-card-default.on { border-color: var(--teal-line); background: var(--teal-tint);
  color: var(--teal); }
.template-card-delete { position: absolute; top: 0.5rem; left: 0.5rem; height: 1.125rem;
  min-width: 1.125rem; padding: 0 0.25rem; display: flex; align-items: center;
  justify-content: center; border-radius: var(--r-badge); border: 1px solid var(--hairline);
  background: rgba(255,255,255,.92); color: var(--muted);
  font-size: 0.7rem; line-height: 1; box-shadow: none; opacity: 0; }
.template-card:hover .template-card-delete, .template-card:focus-within .template-card-delete,
.template-card-delete.confirm { opacity: 1; }
.template-card-delete:hover:not(:disabled) { border-color: var(--danger); color: var(--danger);
  background: rgba(255,255,255,.92); }
.template-card-delete.confirm { width: auto; border-color: var(--danger);
  background: var(--danger); color: #FFFFFF; font-family: var(--mono); font-size: 0.6rem; }
.model-line { display: flex; align-items: center; gap: 0.75rem; font-size: 0.75rem;
  color: var(--ink-2); }
@keyframes status-pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.25; } }
.conversation button.primary { align-self: flex-start; }
/* Candidate cards. Every card shares one stage height; the frame width
   follows the canvas ratio, set per card as --frame-width. */
.canvas-tabs { display: inline-flex; align-self: flex-start; gap: 0.125rem; padding: 0.1875rem;
  margin-bottom: 0.75rem; border: 1px solid var(--hairline); border-radius: var(--r-panel);
  background: var(--sunken); }
button.canvas-tab { display: inline-flex; align-items: baseline; gap: 0.4rem; padding: 0.3rem 0.75rem;
  border: 1px solid transparent; border-radius: var(--r-button); background: transparent;
  box-shadow: none; font-family: var(--mono); font-size: 0.75rem; color: var(--muted); }
button.canvas-tab:hover:not(:disabled) { color: var(--ink); background: transparent;
  border-color: transparent; }
button.canvas-tab.open { background: var(--raised); color: var(--ink); border-color: var(--hairline);
  box-shadow: var(--sh-control); }
.canvas-tab .tab-count { font-size: 0.68rem; color: var(--faint); }
.canvas-grid { display: flex; flex-wrap: wrap; align-items: flex-start; gap: 1rem; min-width: 0; }
.canvas-card { --frame-width: 21.6rem; position: relative; display: flex; flex-direction: column;
  box-sizing: content-box; width: var(--frame-width); border: 1px solid var(--line-soft);
  border-radius: var(--r-card); overflow: hidden; background: var(--raised); cursor: pointer;
  box-shadow: var(--sh-card); transition: border-color 120ms ease, box-shadow 120ms ease; }
.canvas-card:hover { border-color: #B4B0A7; }
.canvas-card.chosen, .canvas-card.chosen:hover { border: 1.5px solid var(--teal);
  box-shadow: 0.5rem 0.5rem 0 0 rgba(14,110,99,.12), var(--sh-card); }
/* A phone card is as wide as `Candidate 12` plus the Finish icon need, or as wide as its bezel; the bezel sits centred in it. */
.canvas-card.phone { width: max(8.75rem, calc(var(--frame-width) + 2rem)); }
.canvas-card.placeholder { cursor: default; }
.card-stage { position: relative; height: var(--stage-height, 13.5rem); overflow: hidden; background: var(--sunken);
  border-bottom: 1px solid var(--hairline); }
.canvas-card:focus-visible { outline: 2px solid var(--teal); outline-offset: 2px; }
.card-arrow { position: absolute; top: 50%; z-index: 3; width: 1.75rem; height: 1.75rem;
  margin-top: -0.875rem; padding: 0; border: 1px solid var(--hairline); border-radius: 999px;
  background: rgba(255,255,255,.96); color: var(--ink-2); font-size: 1rem; line-height: 1;
  display: flex; align-items: center; justify-content: center; box-shadow: var(--sh-control);
  opacity: 0; transition: opacity 0.15s; }
.card-arrow.left { left: 0.5rem; }
.card-arrow.right { right: 0.5rem; }
.card-arrow:disabled { display: none; }
.card-arrow:hover:not(:disabled) { color: var(--teal); border-color: var(--hairline); background: #FFFFFF; }
/* Fine pointers get the arrows on hover; touch screens see them all the time. */
@media (hover: hover) and (pointer: fine) {
  .canvas-card:hover .card-arrow, .canvas-card:focus-visible .card-arrow { opacity: 1; }
}
@media (hover: none) { .card-arrow { opacity: 1; } }
.card-stage iframe, .card-blank { display: block; width: 100%; height: 100%; border: 0;
  pointer-events: none; background: #0B0F14; }
.canvas-card.phone .card-stage { display: flex; align-items: center; justify-content: center;
  box-sizing: border-box; padding: 0.25rem 0; }
.bezel { flex: none; box-sizing: content-box; width: var(--frame-width); height: var(--bezel-height, 12rem);
  padding: 0.5rem; border-radius: 1.375rem; background: #15181C; overflow: hidden;
  box-shadow: 0 0 0 1px rgba(21,24,28,.15); }
.bezel iframe, .bezel .card-blank { width: 100%; height: 100%; border-radius: 0.875rem; }
/* The tick box owns the top-left corner, so the pills start to its right. */
.card-pills { position: absolute; top: 0.5rem; left: 0.5rem; z-index: 2; display: flex;
  align-items: center; gap: 0.375rem; }
.canvas-card.phone .card-pills { top: 0.5rem; left: 0.5rem; flex-direction: column;
  align-items: flex-start; }
/* A selected card wears the accent on its border and its name. The
   footer is the control: a click on it ticks the card. */
.canvas-card.selected, .canvas-card.selected:hover { border-color: var(--teal);
  box-shadow: 0 0 0 1px var(--teal); }
.canvas-card.selected .card-footer { background: var(--teal-tint); }
.card-tick { display: flex; color: var(--teal); }
.card-footer { cursor: pointer; }
.card-footer:hover { background: var(--sunken); }
/* The bulk actions for the ticked cards. */
.selection-bar { display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.75rem;
  padding: 0.375rem 0.5rem; border: 1px solid var(--hairline); border-radius: var(--r-panel);
  background: var(--sunken); }
/* One line always: a wrapped label made the bar taller and moved the
   canvas on the first tick. */
.selection-count { font-family: var(--mono); font-size: 0.72rem; color: var(--ink-2);
  margin-right: auto; white-space: nowrap; }
/* An empty selection keeps the bar's shape: the buttons stay in place,
   greyed, so nothing on the canvas moves on the first tick. */
.selection-bar button:disabled { opacity: 0.45; cursor: default; }
.selection-delete:disabled, .selection-delete:disabled:hover { background: var(--raised);
  border-color: var(--line); color: var(--muted); }
.selection-delete { padding: 0.3rem 0.75rem; border: 1px solid var(--danger);
  border-radius: var(--r-button); background: var(--raised); color: var(--danger);
  font-family: var(--mono); font-size: 0.72rem; }
.selection-delete:hover:not(:disabled), .selection-delete.confirm { background: var(--danger);
  border-color: var(--danger); color: #FFFFFF; }
.selection-clear { padding: 0.3rem 0.75rem; border: 1px solid var(--hairline);
  border-radius: var(--r-button); background: var(--raised); color: var(--ink-2);
  font-family: var(--mono); font-size: 0.72rem; }
.selection-finish, .selection-fork { display: inline-flex; align-items: center;
  gap: 0.35rem; padding: 0.3rem 0.75rem; border: 1px solid var(--teal-line);
  border-radius: var(--r-button); background: var(--raised); color: var(--teal);
  font-family: var(--mono); font-size: 0.72rem; }
.selection-finish span { display: flex; }
.selection-finish:hover:not(:disabled), .selection-fork:hover:not(:disabled) { background: var(--teal-tint); border-color: var(--teal); }
.selection-finish:disabled, .selection-finish:disabled:hover,
.selection-fork:disabled, .selection-fork:disabled:hover { background: var(--raised);
  border-color: var(--line); color: var(--muted); }
.card-pill { display: inline-flex;
  align-items: center; gap: 0.375rem; padding: 0.2rem 0.6rem; border-radius: 999px;
  border: 1px solid var(--hairline); background: rgba(255,255,255,.96); color: var(--teal);
  font-family: var(--mono); font-size: 0.68rem; box-shadow: var(--sh-control); }
.card-pill .dot { width: 6px; height: 6px; border-radius: 999px; background: var(--teal); }
.card-count { flex: none; color: var(--ink-2); font-family: var(--mono); font-size: 0.68rem; }
.card-progress { position: absolute; left: 0; right: 0; bottom: 0; height: 3px; z-index: 3;
  background: rgba(255,255,255,.18); }
.card-progress-fill { height: 100%; background: var(--teal); transition: width 0.4s ease; }
.card-progress.indeterminate .card-progress-fill { width: 35%; animation: finish-slide 1.4s ease-in-out infinite; }
@keyframes finish-slide { 0% { transform: translateX(-100%); } 100% { transform: translateX(300%); } }
.card-footer { display: flex; align-items: center; gap: 0.5rem;
  min-height: 1.75rem; padding: 0.5rem 0.625rem; }
.card-name { flex: 1 1 auto; display: flex; align-items: center; gap: 0.5rem; min-width: 0; }
.canvas-card.phone .card-footer { padding: 0.375rem 0.5rem; min-height: 0; gap: 0.375rem; }
.chosen-pill { flex: none; display: inline-flex; align-items: center; gap: 0.3rem;
  padding: 0.2rem 0.6rem; border: 1px solid var(--teal-line); border-radius: 999px;
  background: var(--teal-tint); color: var(--teal); font-family: var(--mono); font-size: 0.68rem;
  box-shadow: var(--sh-control); }
.chosen-pill span { display: flex; }
.card-number { flex: none; min-width: 1.25rem; padding: 0.05rem 0.4rem; border-radius: var(--r-badge);
  background: var(--sunken); color: var(--ink-2); font-family: var(--mono); font-size: 0.68rem;
  line-height: 1.5; text-align: center; }
.canvas-card.selected .card-number { background: var(--teal); color: #FFFFFF; }
.card-title { flex: 1 1 auto; min-width: 0; font-size: 0.8rem; font-weight: 500; color: var(--ink);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.canvas-card.selected .card-title { color: var(--teal); }
.card-label { font-family: var(--mono); font-size: 0.75rem; color: var(--ink);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.template-name { font: inherit; font-size: 0.82rem; padding: 0.45rem 0.65rem;
  border: 1px solid var(--line); border-radius: var(--r-button); background: var(--raised);
  min-width: 11rem; box-sizing: border-box; width: 100%; }
.progress-track { height: 4px; margin: 0; border-radius: 999px;
  background: #EDEBE5; overflow: hidden; }
.progress-fill { height: 100%; background: var(--teal); transition: width 0.4s ease; }

/* Editor */
.editor { display: grid; grid-template-columns: 400px 1fr;
  align-items: stretch; height: calc(100vh - 3.6rem); position: relative; }
.editor-chat { border-right: 1px solid var(--hairline); background: var(--raised);
  overflow: hidden; height: calc(100vh - 3.6rem); box-sizing: border-box; }
.editor-chat .thread { overflow-y: auto; }
.mention-menu { display: flex; flex-direction: column; gap: 0.125rem; padding: 0.25rem;
  border: 1px solid var(--line); border-radius: var(--r-control); background: var(--raised);
  max-height: 12rem; overflow: auto; }
.mention-item { display: flex; align-items: center; gap: 0.5rem; text-align: left; border: 0;
  background: transparent; color: var(--ink); padding: 0.3rem 0.5rem; border-radius: 5px;
  font-size: 0.8rem; box-shadow: none; }
.mention-item .mono { color: var(--muted); font-size: 0.7rem; }
.mention-item.active, .mention-item:hover { background: var(--teal-tint); }
.context-chip { display: flex; align-items: center; gap: 0.4rem; align-self: flex-start;
  margin: 0.6rem 0.8rem 0; padding: 0.2rem 0.5rem; border-radius: 999px;
  background: var(--teal-tint); color: var(--teal); font-size: 0.68rem; }
.context-chip button { border: 0; background: transparent; color: inherit; padding: 0 0.1rem;
  box-shadow: none; }
.context-row { display: flex; align-items: center; gap: 0.4rem; margin: 0.6rem 0.8rem 0; }
.context-row .context-chip { margin: 0; min-width: 0; }
.context-row .context-chip span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.comment-list { display: flex; flex-direction: column; gap: 0.25rem; margin: 0.6rem 0.8rem 0; }
.comment-head { font-family: var(--mono); font-size: 0.66rem; color: var(--muted);
  text-transform: uppercase; letter-spacing: 0.04em; }
.comment-row { display: flex; align-items: flex-start; gap: 0.4rem; padding: 0.25rem 0.5rem;
  border-radius: 6px; background: var(--teal-tint); color: var(--teal); font-size: 0.68rem; }
.comment-text { flex: 1; min-width: 0; white-space: pre-wrap; overflow-wrap: anywhere; }
.comment-row button { border: 0; background: transparent; color: inherit; padding: 0 0.1rem;
  box-shadow: none; }
.queue-comment { padding: 0.25rem 0.6rem; border: 1px solid var(--teal-line);
  border-radius: var(--r-button); background: var(--raised); color: var(--teal);
  font-family: var(--mono); font-size: 0.68rem; white-space: nowrap; }
.queue-comment:disabled { border-color: var(--line); color: var(--muted); }
.editor-toolbar { position: relative; display: flex; align-items: center; gap: 0.5rem;
  flex-wrap: nowrap; white-space: nowrap; padding: 0.625rem 1rem;
  background: var(--subtle); border-bottom: 1px solid #E3E1DB; }
.editor-toolbar > * { flex: none; }
.editor-toolbar .back { display: inline-flex; align-items: center; gap: 0.25rem;
  font-size: 0.78rem; color: var(--ink-2); padding: 0.375rem 0.625rem 0.375rem 0.4375rem; }
.editor-toolbar .back span { display: flex; }
.editor-toolbar .preview-modes { margin-bottom: 0; align-self: center; }
.editor-toolbar .preview-heading { font-family: var(--mono); font-size: 0.72rem;
  letter-spacing: 0.04em; text-transform: none; color: var(--ink-2); }
.editor-toolbar .save-state { display: inline-flex; align-items: center; gap: 0.25rem;
  font-family: var(--mono); font-size: 0.6875rem; color: var(--faint); }
.editor-toolbar .save-state span { display: flex; }
.save-state.pending { color: var(--muted); animation: status-pulse 1.2s ease-in-out infinite; }
.editor-toolbar button.primary { padding: 0.4375rem 0.75rem; font-size: 0.78rem; }
.editor-toolbar .actions { margin-left: auto; display: flex; align-items: center;
  gap: 0.4375rem; }
.editor-toolbar .actions button { font-size: 0.78rem; }
.export-group { display: flex; align-items: stretch; overflow: hidden;
  border: 1px solid var(--line); border-radius: var(--r-button);
  background: var(--raised); box-shadow: var(--sh-control); }
.export-group > * + * { border-left: 1px solid var(--hairline-2); }
.export-group .button { border: 0; border-radius: 0; box-shadow: none; background: transparent;
  padding: 0.4375rem 0.6875rem; font-size: 0.78rem; color: var(--ink-2);
  display: inline-flex; align-items: center; gap: 0.375rem; }
.export-group .button:first-child { color: var(--ink); }
.export-group .button span { display: flex; }
.export-group .button:hover { background: var(--subtle); border-color: transparent; }
.export-group .button:focus-visible { outline-offset: -2px; }
.export-group .export-cell { display: flex; }
.export-group .button[aria-disabled='true'] { color: #B4B0A7; pointer-events: none; }
.export-group .export-scope { margin: 0.1875rem; align-self: center; }
.export-scope .canvas-tab { padding: 0.2rem 0.6rem; }
.toolbar-more { width: 2rem; height: 2rem; padding: 0; display: flex;
  align-items: center; justify-content: center; font-size: 0.9rem; letter-spacing: 0.05em; }
.menu-backdrop { position: fixed; inset: 0; z-index: 5; }
.toolbar-menu { position: absolute; right: 1rem; top: 3.25rem; z-index: 6; min-width: 12rem;
  background: var(--raised); border: 1px solid #E0DDD6; border-radius: var(--r-panel);
  box-shadow: 0 18px 40px -30px rgba(21,24,28,.5); padding: 0.25rem;
  display: flex; flex-direction: column; gap: 0.25rem; }
.toolbar-menu > button { display: block; width: 100%; text-align: left; border: 0;
  background: transparent; box-shadow: none; font-size: 0.8125rem; padding: 0.5rem 0.75rem;
  border-radius: var(--r-control); }
.toolbar-menu > button:hover:not(:disabled) { background: var(--subtle); }
.toolbar-menu .template-name { margin: 0.25rem; width: calc(100% - 0.5rem); }
.toolbar-menu .menu-actions { display: flex; gap: 0.375rem; padding: 0 0.25rem 0.25rem; }
.toolbar-menu .menu-actions button.primary { padding: 0.4rem 0.7rem; font-size: 0.78rem; }
.properties-sheet { position: absolute; top: 0; right: 0; bottom: 0; width: min(30rem, 90vw);
  background: var(--raised); border-left: 1px solid #E0DDD6;
  box-shadow: -20px 0 40px -30px rgba(21, 24, 28, 0.5);
  display: flex; flex-direction: column; z-index: 5; }
.sheet-head { display: flex; align-items: center; gap: 0.6rem; padding: 0.875rem 1.125rem;
  background: var(--subtle); border-bottom: 1px solid var(--hairline); }
.sheet-head .spacer { margin-left: auto; }
.sheet-head button.primary { padding: 0.45rem 0.85rem; font-size: 0.78rem; }
.sheet-body { flex: 1; overflow-y: auto; overflow-x: hidden; padding: 1.125rem;
  display: flex; flex-direction: column; gap: 1.375rem; min-width: 0; }
.sheet-section { display: flex; flex-direction: column; gap: 0.625rem; min-width: 0; }
.sheet-section > .head { display: flex; align-items: center; gap: 0.5rem;
  font-family: var(--mono); font-size: 0.6875rem; letter-spacing: 0.1em;
  text-transform: uppercase; color: var(--muted); white-space: nowrap; }
.sheet-section > .head::after { content: ''; flex: 1; height: 1px; background: var(--hairline-2); }
.sheet-section > .head.mono-head { text-transform: none; letter-spacing: 0;
  overflow: hidden; text-overflow: ellipsis; }
.sheet-body .theme-grid > *, .sheet-body .frame-grid > * { min-width: 0; }
.sheet-body input, .sheet-body textarea, .sheet-body select { width: 100%; min-width: 0;
  box-sizing: border-box; font: inherit; font-size: 0.84rem; color: var(--ink);
  border: 1px solid var(--line); border-radius: var(--r-button); padding: 0.5625rem 0.6875rem;
  background: var(--raised); box-shadow: inset 0 1px 1px rgba(21,24,28,.03); }
.sheet-body label, .sheet-body .field { display: flex; flex-direction: column; gap: 0.375rem;
  font-size: 0.75rem; color: var(--muted); min-width: 0; }
.sheet-body textarea { min-height: 5rem; font-family: var(--mono); font-size: 0.75rem; }
.history-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.375rem; }
.history-row { display: flex; align-items: center; gap: 0.5rem; font-size: 0.78rem; min-width: 0; }
.history-row .mono { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.history-row button { padding: 0.3rem 0.6rem; font-size: 0.72rem; }
.theme-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 0.9rem; }
.font-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 0.625rem; }
.font-grid .select-trigger { padding: 0.5rem 0.5625rem; font-size: 0.8rem; gap: 0.3rem; }
.screen-actions { display: flex; gap: 0.45rem; }
.frame-grid { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 0.6rem; }
.frame-grid label { font-size: 0.7rem; }
.frame-grid input[type='checkbox'] { margin-right: 0.3rem; }
.inspector-empty { border: 1px dashed var(--line-soft); border-radius: 9px;
  background: var(--subtle); padding: 0.875rem; display: flex; gap: 0.5625rem;
  align-items: center; }
.inspector-empty .glyph { display: flex; color: var(--faint); flex: none; }
.inspector-hint { margin: 0; font-size: 0.78rem; line-height: 1.5; color: var(--muted); }
.color-list { border: 1px solid var(--hairline); border-radius: 9px; overflow: hidden; }
.color-list .color-field { display: flex; flex-direction: row; align-items: center;
  gap: 0.625rem; padding: 0.625rem 0.75rem; cursor: pointer; }
.color-list .color-field + .color-field { border-top: 1px solid #F0EEE9; }
.color-list input[type='color'] { width: 1.375rem; height: 1.375rem; padding: 0; border: 0;
  border-radius: 5px; box-shadow: inset 0 0 0 1px rgba(21,24,28,.12); cursor: pointer;
  flex: none; }
.color-list .color-name { font-size: 0.8125rem; color: var(--ink); }
.color-list .color-code { margin-left: auto; font-size: 0.72rem; color: var(--muted); }
.kind-badge { font-family: var(--mono); font-size: 0.62rem; letter-spacing: 0.04em;
  color: var(--muted); border: 1px solid var(--hairline); border-radius: 4px;
  padding: 0.05rem 0.35rem; }
/* The kind picker: three cards, asked once when the request is sent. The base button
   rule forbids wrapping, so the card allows it again and the detail line folds. */
.kind-modal { width: min(38rem, calc(100vw - 2rem)); }
.kind-choices { display: grid; gap: 0.625rem; }
button.kind-choice { display: flex; flex-direction: column; align-items: stretch; gap: 0.25rem;
  width: 100%; min-width: 0; padding: 0.9rem 1rem; text-align: left; white-space: normal;
  border: 1px solid var(--line); border-radius: var(--r-panel); background: var(--raised);
  box-shadow: none; }
button.kind-choice:hover { border-color: var(--ink); background: var(--subtle); }
.kind-choice-name { font-size: 0.95rem; font-weight: 500; color: var(--ink); }
.kind-choice-detail { font-size: 0.78rem; color: var(--muted); line-height: 1.4; }
.kind-note { margin-top: 0.75rem; font-size: 0.74rem; color: var(--muted); }
/* The canvas picker: three device buttons, any number pressed. */
.canvas-picker { display: flex; flex-direction: column; gap: 0.45rem; }
.device-choices { display: flex; gap: 0.5rem; }
button.device-choice { display: flex; flex-direction: column; align-items: center; gap: 0.15rem;
  flex: 1; padding: 0.6rem 0.4rem; border: 1px solid var(--line); border-radius: var(--r-panel);
  background: var(--raised); color: var(--ink-2); box-shadow: none; }
button.device-choice:hover { border-color: #B4B0A7; color: var(--ink); }
button.device-choice.picked { border-color: var(--ink); background: var(--subtle);
  color: var(--ink); box-shadow: inset 0 0 0 1px var(--ink); }
.device-glyph { display: flex; }
.device-name { font-size: 0.78rem; font-weight: 500; }
.device-size { font-family: var(--mono); font-size: 0.62rem; color: var(--muted); }
.device-note { margin: 0; font-size: 0.7rem; color: var(--muted); }

.count-chips { display: flex; align-items: center; gap: 0.5rem; }
.count-chips-label { font-family: var(--mono); font-size: 0.62rem; letter-spacing: 0.08em;
  text-transform: uppercase; color: var(--faint); }
.count-chips .effect-chips button { min-width: 1.8rem; padding: 0.3rem 0.55rem;
  justify-content: center; }
.kind-field { margin-bottom: 0.75rem; }
/* The app's deck, document, social, print, mailing, campaign, and artwork questions: cards that look like the model's questions. */
.deck-questions, .document-questions, .social-questions, .print-questions, .mailing-questions, .campaign-questions, .artwork-questions { display: grid; gap: 0.75rem; grid-template-columns: repeat(auto-fit, minmax(20rem, 1fr));
  align-items: start; }
.app-question { display: flex; flex-direction: column; gap: 0.6rem; padding: 0.9rem 1rem; }
/* The cards mix short and tall: fill the rows, and keep every card at
   its own height, aligned to the top of its row. */
.workbench-questions .with-app-questions .question-cards { grid-auto-flow: dense; align-items: start; }
.project-kind { font-family: 'JetBrains Mono', ui-monospace, monospace; font-size: 0.72rem;
  color: #6C7178; white-space: nowrap; }
.effect-chips { display: flex; flex-wrap: wrap; gap: 0.375rem; }
.effect-chips button { border-radius: 999px; padding: 0.375rem 0.8125rem; font-size: 0.78rem;
  color: var(--ink-2); box-shadow: none; }
.effect-chips button.selected { background: var(--ink); border-color: var(--ink);
  color: var(--paper); }

/* Uploads panel */
.attach-button { position: relative; overflow: hidden; display: inline-flex; align-items: center;
  justify-content: center; width: var(--track); height: var(--track); flex: none;
  box-sizing: border-box; border: 1px solid var(--line); border-radius: var(--r-track);
  background: var(--raised);
  color: var(--ink-2); line-height: 1; cursor: pointer;
  box-shadow: var(--sh-control); transition: background-color 120ms ease, border-color 120ms ease; }
.attach-button span { display: flex; }
.attach-button:hover { background: var(--subtle); border-color: #B4B0A7; color: var(--ink); }
.attach-button.busy { color: var(--faint); cursor: default; }
.attach-button input[type='file'] { position: absolute; inset: 0; opacity: 0; cursor: pointer; }
.brief-attachments { list-style: none; margin: 0; padding: 0 1.5rem 0.8rem; display: flex;
  flex-wrap: wrap; gap: 0.4rem; }
.attachment-chip { display: inline-flex; align-items: center; gap: 0.4rem; font-size: 0.72rem;
  padding: 0.3125rem 0.375rem 0.3125rem 0.6875rem; border: 1px solid var(--hairline);
  border-radius: 999px; background: var(--subtle); max-width: 100%; }
.attachment-chip .mono { overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  max-width: 16rem; }
.attachment-size { color: var(--faint); font-size: 0.68rem; white-space: nowrap; }
.attachment-remove { width: 17px; height: 17px; padding: 0; border: 0; border-radius: 999px;
  background: #EDEBE5; color: var(--muted); font-size: 0.8rem; line-height: 1;
  display: flex; align-items: center; justify-content: center; box-shadow: none; }
.attachment-remove:hover:not(:disabled) { background: #EDEBE5; color: var(--danger); }

/* Preview pane */
.editor-preview { background: var(--sunken); display: flex; flex-direction: column;
  min-width: 0; min-height: 0; overflow: hidden; }
.editor-body { flex: 1; min-height: 0; overflow-y: auto; display: flex; flex-direction: column;
  gap: 0.75rem; padding: 1.125rem 1.25rem; }
.preview-stage { position: relative; flex: none; display: flex; align-items: center;
  justify-content: center; }
.preview-stage iframe { display: block; width: 100%; border: 0; border-radius: var(--r-primary);
  background: var(--raised); box-shadow: 0 20px 44px -30px rgba(21,24,28,.6), 0 0 0 1px #DAD7D0; }
.preview-stage.narrow iframe { width: auto; height: 70vh; max-width: 100%; }
.preview-stage.phone { padding: 1.75rem; border-radius: var(--r-card); background-color: var(--sunken);
  background-image: radial-gradient(circle, var(--line-soft) 1px, transparent 1.5px);
  background-size: 1rem 1rem; }
.preview-stage.phone iframe { border-radius: 1.5rem; background: #000;
  box-shadow: 0 0 0 0.75rem #15181C, 0 0 0 calc(0.75rem + 1px) rgba(21,24,28,.15),
    0 28px 48px -24px rgba(21,24,28,.7); }
.preview-hint { margin: 0; font-family: var(--mono); font-size: 0.6875rem; color: var(--faint);
  display: flex; align-items: center; gap: 0.875rem; flex-wrap: wrap; }
.preview-hint .dot { color: var(--line); }
.preview-hint kbd { border: 1px solid #E3E1DB; background: var(--raised);
  border-radius: var(--r-badge); padding: 1px 5px; color: var(--muted); font: inherit; }
/* The thumbnail strip. Every tile is 5.5rem tall; its width follows the
   canvas ratio, set per tile as --tile-width. */
.strip-head { display: flex; align-items: center; gap: 0.75rem; font-family: var(--mono);
  font-size: 0.7rem; letter-spacing: 0.1em; text-transform: uppercase; color: var(--muted); }
.strip-head .strip-counts { letter-spacing: 0; text-transform: none; color: var(--faint); }
.strip-head::after { content: ''; flex: 1; height: 1px; background: var(--hairline); }
.thumbnails { display: flex; flex-wrap: wrap; align-items: stretch; gap: 0.625rem; }
.thumbnail { --tile-width: 8.8rem; position: relative; flex: none; width: var(--tile-width);
  height: 5.5rem; box-sizing: border-box; padding: 0; overflow: hidden; border: 1px solid #DAD7D0;
  border-radius: var(--r-control); background: var(--raised); cursor: grab; user-select: none;
  -webkit-user-select: none; touch-action: none;
  transition: box-shadow 120ms ease, border-color 120ms ease; }
.thumbnail iframe { position: absolute; top: 0; left: 0; width: calc(var(--tile-width) * 10);
  height: 55rem; border: 0; transform: scale(0.1); transform-origin: top left;
  pointer-events: none; }
.thumbnail.current { border-color: var(--teal); box-shadow: 0 0 0 3px var(--teal-tint); }
.thumbnail.dragging { opacity: 0.6; cursor: grabbing; }
.thumbnail.pinned { border-color: var(--teal); border-style: dashed; }
.thumbnail.pinned .thumbnail-number { background: var(--teal); color: #FFFFFF; }
.thumbnail-number { position: absolute; left: 0.25rem; bottom: 0.25rem; z-index: 1;
  padding: 0.05rem 0.4rem; border-radius: var(--r-badge); background: var(--sunken);
  color: var(--ink); font-family: var(--mono); font-size: 0.6rem; line-height: 1.5; }
.thumbnail.current .thumbnail-number { background: var(--teal); color: #FFFFFF; }
.thumbnail-delete { position: absolute; top: 0.25rem; right: 0.25rem; z-index: 2; display: none;
  align-items: center; justify-content: center; gap: 0.3rem; width: 1.375rem; height: 1.375rem;
  padding: 0; border: 1px solid var(--hairline); border-radius: 999px;
  background: rgba(255,255,255,.96); color: var(--ink-2); font-size: 0.8rem; line-height: 1;
  box-shadow: var(--sh-control); }
.thumbnail:hover .thumbnail-delete, .thumbnail-delete.confirm { display: inline-flex; }
.thumbnail.deleting { border-color: var(--danger); }
.thumbnail.deleting::after { content: ''; position: absolute; inset: 0; z-index: 1;
  background: rgba(180,35,31,.55); }
.thumbnail-delete.confirm { top: 50%; left: 50%; right: auto; transform: translate(-50%, -50%);
  width: auto; height: auto; padding: 0.3rem 0.7rem; border-color: var(--danger);
  background: var(--danger); color: #FFFFFF; font-family: var(--mono); font-size: 0.7rem;
  white-space: nowrap; }
.thumbnail.portrait .thumbnail-delete.confirm { width: 1.5rem; height: 1.5rem; padding: 0; }
.thumbnail.portrait .thumbnail-delete.confirm .delete-text { display: none; }
/* Redo sits left of Delete and takes two clicks the same way. */
.thumbnail-redo { position: absolute; top: 0.25rem; right: 1.85rem; z-index: 2; display: none;
  align-items: center; justify-content: center; gap: 0.3rem; width: 1.375rem; height: 1.375rem;
  padding: 0; border: 1px solid var(--hairline); border-radius: 999px;
  background: rgba(255,255,255,.96); color: var(--ink-2); line-height: 1;
  box-shadow: var(--sh-control); }
.thumbnail-redo span { display: flex; }
.thumbnail:hover .thumbnail-redo, .thumbnail-redo.confirm { display: inline-flex; }
.thumbnail-redo.confirm { top: 50%; left: 50%; right: auto; transform: translate(-50%, -50%);
  width: auto; height: auto; padding: 0.3rem 0.7rem; border-color: var(--teal);
  background: var(--teal); color: #FFFFFF; font-family: var(--mono); font-size: 0.7rem;
  white-space: nowrap; }
.thumbnail.portrait .thumbnail-redo.confirm { width: 1.5rem; height: 1.5rem; padding: 0; }
.thumbnail.portrait .thumbnail-redo.confirm .redo-text { display: none; }
.strip-divider { flex: none; align-self: stretch; width: 1px; margin: 0 0.25rem;
  background: var(--line); }
/* A planned tile is the size of a written tile and reads as a label, not
   a control. On a phone tile only the number fits; the tooltip carries
   the title. */
.thumbnail.outline { cursor: default; display: flex; flex-direction: column; gap: 0.1rem;
  justify-content: space-between; padding: 0.3rem 0.4rem; text-align: left; font: inherit;
  box-shadow: none; border: 1px dashed var(--teal-line); border-radius: var(--r-control);
  background: var(--teal-tint); }
.strip-write { display: inline-flex; align-items: center; gap: 0.35rem; padding: 0.2rem 0.6rem;
  border: 1px solid var(--teal-line); border-radius: var(--r-badge); background: var(--raised);
  color: var(--teal); font-family: var(--mono); font-size: 0.65rem; letter-spacing: 0;
  text-transform: none; cursor: pointer; }
.strip-write:hover { background: var(--teal-tint); border-color: var(--teal); }
.strip-write span { display: flex; }
.outline-kicker { font-family: var(--mono); font-size: 0.6rem; color: var(--teal);
  white-space: nowrap; }
.outline-title { font-size: 0.7rem; font-weight: 500; line-height: 1.25; color: var(--ink);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.thumbnail.outline.portrait { padding: 0.25rem 0.15rem; align-items: center; }
.thumbnail.outline.portrait .planned-text,
.thumbnail.outline.portrait .outline-title { display: none; }
.notes-box { display: flex; flex-direction: column; gap: 0.375rem; }
.notes-heading { font-family: var(--mono); font-size: 0.7rem;
  letter-spacing: 0.1em; text-transform: uppercase; color: var(--muted); }
.notes-heading .screen-no { color: var(--faint); letter-spacing: 0; text-transform: none;
  margin-left: 0.5rem; }
.notes-box textarea { font: inherit; font-size: 0.84rem; line-height: 1.55; min-height: 4.5rem;
  border: 1px solid #DAD7D0; border-radius: var(--r-primary); padding: 0.6875rem 0.8125rem;
  background: var(--raised); box-shadow: 0 1px 2px rgba(21,24,28,.03); }
.color-field { display: flex; align-items: center; gap: 0.5rem; }
.color-field input[type='color'] { width: 2.4rem; height: 1.8rem; padding: 0.1rem;
  border: 1px solid var(--line); border-radius: var(--r-badge); background: var(--raised);
  cursor: pointer; }
.color-code { font-size: 0.7rem; color: var(--muted); }

/* Brief composer */
.brief-card { background: var(--raised); border: 1px solid var(--line-soft);
  border-radius: var(--r-card); display: flex; flex-direction: column; overflow: hidden;
  box-shadow: var(--sh-float); }
.brief-card textarea { border: 0; outline: none; resize: none;
  font: inherit; font-size: 1rem; line-height: 1.6; color: var(--ink);
  padding: 1.375rem 1.5rem 0.5rem; min-height: 7.25rem; }
.brief-card textarea::placeholder { color: var(--ghost); }
.brief-footer { display: flex; flex-wrap: nowrap; align-items: center;
  justify-content: space-between; gap: 0.75rem; border-top: 1px solid var(--hairline-2);
  background: var(--subtle); padding: 0.7rem 0.75rem 0.7rem 0.875rem; }
.brief-footer button.primary { flex: none; }

/* Agent status bar and field badges */
.agent-bar { display: flex; align-items: center; justify-content: space-between;
  gap: 1rem; padding: 0.55rem 1.75rem; background: #14171B; color: #B4BAC1;
  font-family: var(--mono); font-size: 0.72rem; }
.agent-bar .accent { color: #7FBFB4; }
.field-heading { display: flex; align-items: center; justify-content: space-between;
  gap: 0.5rem; }
.badge { font-family: var(--mono); font-size: 0.625rem;
  letter-spacing: 0.08em; text-transform: uppercase; color: var(--muted);
  border: 1px solid #E3E1DB; background: var(--paper); border-radius: var(--r-badge);
  padding: 0.1rem 0.35rem; white-space: nowrap; }
.badge.you { color: var(--teal); border-color: var(--teal-line); background: var(--teal-tint); }

/* Agent questions */
.question-panel { width: 100%; display: flex;
  flex-direction: column; gap: 0.8rem; margin-bottom: 0.6rem; }
.question-grid { display: grid; gap: 0.8rem;
  grid-template-columns: repeat(auto-fit, minmax(24rem, 1fr)); }
.question-card.wide { grid-column: 1 / -1; }
/* Two cards that share one row at half width each. They fold to one
   column when the row is too narrow for both. */
.question-pair { grid-column: 1 / -1; display: grid; gap: 0.75rem; align-items: start;
  grid-template-columns: repeat(auto-fit, minmax(20rem, 1fr)); }
.question-panel button.primary { align-self: flex-start; }
.question-card { background: var(--raised); border: 1px solid #E0DDD6; border-radius: var(--r-card);
  padding: 1.1rem 1.25rem; display: flex; flex-direction: column; gap: 0.8rem;
  box-shadow: var(--sh-card); }
.question-row { display: flex; align-items: baseline; gap: 0.7rem; }
.question-number { font-family: var(--mono); font-size: 0.72rem; color: var(--faint); }
.question-text { font-size: 0.95rem; font-weight: 500; }
.option-chips { display: flex; flex-wrap: wrap; gap: 0.45rem; }
.option-chip { border-radius: 999px; padding: 0.375rem 0.8125rem; font-size: 0.78rem;
  color: var(--ink-2); box-shadow: none; }
.option-chip.selected { background: var(--ink); border-color: var(--ink); color: var(--paper); }
.option-chip.selected:hover:not(:disabled) { background: #23272C; border-color: #23272C; }
.option-chip.skip { border-style: dashed; border-color: var(--line); color: var(--muted); }
/* Picked, the skip chip takes the dark fill from `.selected`. This rule
   is needed because `.option-chip.skip` is declared after
   `.option-chip.selected` at the same specificity, so its muted text
   would otherwise stay grey on near-black. The dash stays: it still
   marks an assumption rather than an answer. */
.option-chip.skip.selected { color: var(--paper); border-color: var(--ink); }
.option-chip.other { }
/* The write-in: a dashed chip that opens one text field under the row. */
.option-chip.write-in { border-style: dashed; border-color: var(--line); color: var(--muted); }
.option-chip.write-in:hover:not(:disabled) { border-color: var(--ink); color: var(--ink); }
.write-in-row { display: flex; align-items: center; gap: 0.45rem; margin-top: 0.5rem; }
.write-in-field { flex: 1; min-width: 0; font: inherit; font-size: 0.78rem;
  border: 1px solid var(--line); border-radius: 999px; padding: 0.375rem 0.8125rem;
  background: var(--raised); color: var(--ink); }
.write-in-field:focus { outline: none; border-color: var(--ink); }
.option-chip.write-in-save { flex: none; }
.option-chip.write-in-save:disabled { opacity: .45; cursor: not-allowed; }

/* The session workspace: the chat with the Q&A on the left, the candidates on the right. */
/* minmax(0, 1fr): the workbench must never widen to its content, or WebKit
   sizes a wrapping card grid as one long row and the page scrolls sideways. */
.session { display: grid; grid-template-columns: 420px minmax(0, 1fr); height: calc(100vh - 3.6rem); }
.conversation { display: flex; flex-direction: column; min-height: 0;
  border-right: 1px solid var(--hairline); }
.studio-head { display: flex; align-items: center; gap: 0.75rem; padding: 0.9rem 1.25rem;
  border-bottom: 1px solid var(--hairline); }
.studio-head .back { font-size: 0.82rem; color: var(--ink-2); box-shadow: none; padding: 0.3rem 0.5rem; }
.studio-title { font-size: 0.95rem; font-weight: 600; }
.thread { flex: 1; overflow-y: auto; padding: 1.1rem 1.25rem; display: flex;
  flex-direction: column; gap: 0.75rem; }
.workbench { min-width: 0; background: var(--sunken); overflow-y: auto; overflow-x: hidden; padding: 1.25rem 1.5rem;
  display: flex; flex-direction: column; gap: 1rem; }
.start-run { align-self: flex-start; }

.state-badge { display: inline-flex; align-items: center; border-radius: 999px;
  padding: 0.12rem 0.55rem; font-size: 0.7rem; font-weight: 500; background: var(--subtle);
  border: 1px solid var(--hairline); color: var(--ink-2); }
.state-badge.generating { background: var(--teal-tint); border-color: var(--teal-line); color: var(--teal); }
.state-badge.reviewing { background: var(--teal-tint); border-color: var(--teal-line); color: var(--teal); }
.state-badge.error { color: var(--danger, #b4231f); border-color: rgba(180,35,31,.35); }
/* A stop is not a failure: neutral ink, not the danger red. */
.state-badge.stopped { color: var(--ink-2); border-color: var(--line); background: var(--subtle); }

.question-set { align-self: stretch; background: var(--raised); border: 1px solid var(--line-soft);
  border-radius: 12px; box-shadow: var(--sh-card, 0 1px 2px rgba(21,24,28,.06));
  padding: 0.9rem 1rem; display: flex; flex-direction: column; gap: 0.75rem; }
.question-cards { display: flex; flex-direction: column; gap: 0.75rem; }
/* The open question set sits in the workbench, where the questions
   can share the width. */
.workbench-questions .question-set { padding: 1.1rem 1.25rem; gap: 0.9rem; }
.workbench-questions .question-cards { display: grid; gap: 0.75rem;
  grid-template-columns: repeat(auto-fit, minmax(20rem, 1fr)); align-items: start; }
.workbench-questions .question-card { padding: 0.9rem 1rem; }
.workbench-questions .question-label { font-size: 0.92rem; }
.question-set-actions { display: flex; align-items: center; gap: 0.625rem; }
.question-step { font-size: 0.75rem; color: var(--muted); font-variant-numeric: tabular-nums; }
.question-hint { font-size: 0.72rem; color: var(--muted); }
/* The open question set sits in the chat thread, one column wide. */
.thread-questions { display: flex; flex-direction: column; gap: 0.75rem; }
.thread-questions .question-set-actions { flex-wrap: wrap; }
.thread-questions .canvas-picker { padding: 0.75rem 0.875rem; background: var(--raised);
  border: 1px solid var(--line-soft); border-radius: 12px; }
.thread-answers { display: flex; flex-direction: column; gap: 0.4rem; padding: 0.5rem 0.75rem;
  border: 1px solid var(--hairline); border-radius: 10px; background: var(--subtle); }
.workbench .run-settings { border-top: 0; padding-top: 0; background: var(--raised);
  border: 1px solid var(--line-soft); border-radius: 12px; padding: 1rem 1.1rem; }
.question-card { background: var(--subtle); border-radius: 10px; padding: 0.75rem 0.875rem;
  display: flex; flex-direction: column; gap: 0.5rem; }
.question-head { display: flex; align-items: center; gap: 0.5rem; }
.question-label { font-size: 0.85rem; font-weight: 500; }
/* A pick the planner read from the request: the chip is picked, and the
   tag says why, until the user picks for themselves. */
.suggested-tag { font-size: 0.66rem; font-weight: 500; letter-spacing: 0.02em; color: var(--teal);
  background: var(--teal-tint); border-radius: 999px; padding: 0.1rem 0.45rem; }
.option-chip.selected.suggested { border-style: dashed; }
.question-rationale { font-size: 0.72rem; color: var(--muted); }
.badge.required { font-size: 0.62rem; color: var(--muted); border: 1px solid var(--line);
  border-radius: 4px; padding: 0 0.3rem; }
/* border-box: `width: 100%` plus padding and a border would otherwise
   overflow the card by the padding. */
.answer-textarea, .other-input { width: 100%; box-sizing: border-box; font: inherit;
  font-size: 0.84rem; border: 1px solid var(--line); border-radius: 6px;
  padding: 0.4rem 0.5rem; background: var(--raised); }
/* A question and its answer must never read alike. Each line carries a
   Q or A tag, so the role is legible before the words are. */
.qa-row { display: flex; flex-direction: column; gap: 0.2rem; padding: 0.1rem 0; }
.qa-line { display: flex; align-items: flex-start; gap: 0.45rem; }
.qa-tag { flex: none; width: 1.05rem; height: 1.05rem; margin-top: 0.1rem;
  display: inline-flex; align-items: center; justify-content: center; border-radius: 4px;
  font-family: var(--mono); font-size: 0.58rem; font-weight: 600; letter-spacing: 0;
  background: var(--ghost); color: var(--muted); }
.qa-tag.answer { background: var(--ink); color: var(--raised); }
.qa-question { font-size: 0.78rem; line-height: 1.45; color: var(--muted); }
.qa-answer { font-size: 0.86rem; line-height: 1.45; font-weight: 600; color: var(--ink); }
.qa-answer.assumed { font-weight: 400; font-style: italic; color: var(--muted); }

.brief-answers { display: flex; flex-direction: column; gap: 0.55rem;
  border: 1px solid var(--hairline); border-radius: var(--r-panel); background: var(--subtle);
  padding: 0.7rem 0.85rem; }
.run-settings { display: flex; flex-direction: column; gap: 0.65rem;
  border-top: 1px solid var(--hairline); padding-top: 0.8rem; }
.run-settings .brief-field { margin: 0; }
button.brief-toggle { align-self: flex-start; padding: 0; border: 0; background: transparent;
  box-shadow: none; font-size: 0.75rem; color: var(--muted); text-decoration: underline;
  text-underline-offset: 2px; }
button.brief-toggle:hover { background: transparent; color: var(--ink); }

.brief-panel { background: var(--raised); border: 1px solid var(--line-soft); border-radius: 12px;
  box-shadow: var(--sh-card, 0 1px 2px rgba(21,24,28,.06)); padding: 1rem 1.1rem;
  display: flex; flex-direction: column; gap: 0.75rem; }
.brief-head { display: flex; align-items: center; gap: 0.6rem; width: 100%; padding: 0; border: 0;
  background: transparent; box-shadow: none; text-align: left; cursor: pointer; font: inherit; }
.brief-head:hover { background: transparent; }
.brief-head .chevron { font-size: 0.7rem; color: var(--faint); width: 0.8rem; flex: none; }
.brief-panel.collapsed { gap: 0; }
.brief-head .rev { font-family: var(--mono); font-size: 0.65rem; color: var(--faint); margin-left: auto; }
.brief-groups { display: flex; flex-direction: column; gap: 0.625rem; }
.brief-group { border-radius: 8px; padding: 0.625rem 0.75rem; }
.brief-group-title { font-family: var(--mono); font-size: 0.62rem; text-transform: uppercase;
  letter-spacing: 0.04em; }
.brief-list { margin: 0.3rem 0 0; padding-left: 1.1rem; font-size: 0.8rem; display: flex;
  flex-direction: column; gap: 0.2rem; }
.brief-group.facts { background: var(--teal-tint); border: 1px solid var(--teal-line); }
.brief-group.facts .brief-group-title { color: var(--teal); }
.brief-group.assumptions { background: var(--sunken); border: 1px dashed var(--line); }
.brief-group.assumptions .brief-group-title { color: var(--ink-2); }
.brief-group.open { background: var(--raised); border: 1px solid var(--hairline); }
.brief-group.open .brief-group-title { color: var(--danger, #b4231f); }
.brief-fields { display: flex; flex-direction: column; gap: 0.5rem; }
.brief-field { display: flex; flex-direction: column; gap: 0.15rem; }
.brief-field-label { font-size: 0.68rem; color: var(--muted); text-transform: uppercase;
  letter-spacing: 0.03em; }
.brief-field-value { font-size: 0.85rem; }
.section-row { display: grid; grid-template-columns: 1fr 2fr; gap: 0.5rem; font-size: 0.8rem; }
.section-name { font-weight: 500; }
.section-content { color: var(--ink-2); }
.brief-actions { display: flex; flex-direction: row-reverse; flex-wrap: wrap; gap: 0.625rem; align-items: center; }
.revision-list { display: flex; flex-direction: column; gap: 0.2rem; }
.revision-list button.history-row { justify-content: flex-start; padding: 0.25rem 0.4rem; border: 1px solid transparent;
  border-radius: var(--r-control); background: transparent; box-shadow: none; text-align: left; }
.revision-list button.history-row:hover:not(:disabled) { background: var(--subtle); border-color: transparent; }
.revision-list .history-row.selected { background: var(--teal-tint); border-color: var(--teal-line); }
.revision-list .revision-name { flex: none; font-family: var(--mono); font-size: 0.7rem; color: var(--muted); }
.revision-list .history-row.current .revision-name { color: var(--ink); font-weight: 500; }
.revision-list .revision-summary { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  font-size: 0.74rem; color: var(--ink-2); }
.revision-view { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
.revision-view .kicker { margin-right: auto; }
.revision-view button { padding: 0.3rem 0.6rem; font-size: 0.72rem; }

.progress-step { font-size: 0.8rem; font-weight: 500; margin-bottom: 0.4rem; }

.error-card { background: var(--raised); border: 1px solid rgba(180,35,31,.35); border-radius: 10px;
  padding: 0.75rem 0.875rem; display: flex; flex-direction: column; gap: 0.4rem; align-items: flex-start; }
.error-card .error-title { color: var(--danger, #b4231f); font-weight: 600; }
/* An error names a candidate id, one long token, so it must break. */
.error-card p { margin: 0; max-width: 100%; overflow-wrap: anywhere; }
/* The stopped card mirrors the error card without the alarm. */
.stopped-card { background: var(--raised); border: 1px solid var(--line); border-radius: 10px;
  padding: 0.75rem 0.875rem; display: flex; flex-direction: column; gap: 0.4rem; align-items: flex-start; }
.stopped-card .stopped-title { color: var(--ink); font-weight: 600; }
.stopped-card p { color: var(--ink-2); font-size: 0.86rem; }

.chat-controls { display: flex; align-items: center; gap: 0.5rem; justify-content: flex-end;
  padding: 0.5rem 1.25rem; }
.session-row .state-badge { margin-left: auto; }
";

/// The Swift Design pinwheel, as one SVG path in a 178 by 179 box. The
/// brand mark and the tab icon both draw this path, so the two cannot
/// drift apart.
const LOGO_PATH: &str = "\
M95,99L95,129L94,130L94,136L93,137L93,144L92,145L92,147L91,149L92,149L116,125L116,120ZM89,101L\
87,99L85,99L84,98L82,100L81,100L81,101L79,102L79,103L77,104L77,105L75,106L75,107L73,108L73,109\
L71,110L71,111L69,112L69,113L67,114L67,115L65,116L65,117L63,118L63,119L61,120L61,121L59,122L59\
,123L57,124L57,125L55,126L55,127L53,128L53,129L51,130L51,131L49,132L49,133L47,134L47,135L45,13\
6L45,137L43,138L43,139L41,140L41,141L39,142L39,143L37,144L37,145L35,146L35,147L33,148L33,149L3\
1,150L31,151L29,152L29,153L27,154L27,155L25,156L25,157L23,158L23,159L21,160L21,161L19,162L19,1\
63L17,164L17,165L15,166L14,168L13,168L13,170L16,170L17,171L34,171L35,172L42,172L43,171L58,171L\
59,170L62,170L63,169L67,169L70,167L72,167L74,165L76,165L80,161L80,160L84,155L84,153L86,149L86,\
146L87,145L87,142L88,141L88,132L89,131ZM29,92L29,93L30,93L31,95L32,95L33,97L34,97L35,99L36,99L\
37,101L38,101L39,103L40,103L41,105L42,105L43,107L44,107L45,109L46,109L47,111L48,111L49,113L50,\
113L51,115L52,115L54,117L56,117L58,116L78,96L78,95L47,95L46,94L38,94L37,93L34,93L33,92ZM100,89\
L99,90L98,95L168,165L170,165L170,156L171,155L171,125L170,124L170,116L168,112L168,109L164,101L1\
61,98L160,98L158,96L152,93L150,93L149,92L146,92L145,91L142,91L141,90L134,90L133,89L123,89L122,\
88L107,88L106,89ZM87,86L85,89L86,91L89,93L91,91L92,91L92,88L90,86ZM99,82L99,83L131,83L132,84L1\
37,84L138,85L144,85L148,87L148,85L125,62L120,62L118,64L117,64L117,65L115,66L115,67L113,68L113,\
69L111,70L111,71L109,72L109,73L107,74L107,75L105,76L105,77L103,78L103,79L101,80L100,82ZM86,29L\
62,53L61,55L61,57L63,59L63,60L81,78L83,79L83,46L84,45L84,37L85,36ZM8,13L7,15L7,30L6,31L6,48L7,\
49L7,59L8,60L8,63L9,64L9,67L10,68L11,73L13,75L13,76L19,82L25,85L30,86L31,87L36,87L37,88L40,88L\
41,89L56,89L57,90L66,90L67,89L70,90L71,89L78,89L78,87L80,85L80,84L9,13ZM165,8L157,8L156,7L145,\
7L144,6L134,6L133,7L123,7L122,8L116,8L115,9L108,10L106,12L104,12L102,14L101,14L96,19L93,25L93,\
27L91,31L91,35L90,36L90,39L89,40L89,53L88,54L88,72L89,73L89,78L90,79L92,79L94,80L165,9Z";

/// Builds the pinwheel as one SVG document, `size` pixels square and
/// filled with `fill`.
fn logo_svg(size: u32, fill: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{size}\" height=\"{size}\" \
         viewBox=\"0 0 178 179\" aria-hidden=\"true\">\
         <path fill=\"{fill}\" d=\"{LOGO_PATH}\"></path></svg>"
    )
}

fn main() {
    dioxus::launch(App);
}

/// What the top bar shows beside the brand while a view has a live
/// run: the status sentence and the model that runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TopbarContext {
    /// The status sentence, like `Working… 45%`.
    pub label: String,
    /// `provider/model`, when a model is chosen.
    pub model: Option<String>,
}

/// Root component: top bar plus the current view. The view is kept in
/// the URL hash, so a reload lands on the same screen.
#[component]
fn App() -> Element {
    let mut view = use_signal(|| Option::<View>::None);
    let topbar_context = use_context_provider(|| Signal::new(Option::<TopbarContext>::None));
    // The arrow keys move the caret inside a draft; only on its first
    // or last line do they recall an earlier prompt.
    use_hook(|| {
        document::eval(prompt_history::ARROW_GUARD);
    });
    // Read the hash on load and on every hashchange.
    use_future(move || async move {
        let mut channel = document::eval(route::HASH_LISTENER);
        while let Ok(hash) = channel.recv::<String>().await {
            let next = route::route_from_hash(&hash);
            if view.peek().as_ref() != Some(&next) {
                view.set(Some(next));
            }
        }
    });
    // Write the hash the app chose.
    use_effect(move || {
        if let Some(current) = view() {
            let writer = document::eval(route::WRITE_HASH);
            let _ = writer.send(route::hash_for(&current));
        }
    });
    rsx! {
        style { dangerous_inner_html: STYLESHEET }
        header { class: "topbar",
            button { class: "brand", onclick: move |_| view.set(Some(View::Home)),
                span { dangerous_inner_html: logo_svg(20, "currentColor") }
                span { "Swift Design" }
            }
            if let Some(context) = topbar_context() {
                div { class: "topbar-context",
                    span { class: "status-dot" }
                    span { "{context.label}" }
                    if let Some(model) = context.model {
                        span { "{model}" }
                    }
                }
            }
        }
        match view() {
            None => rsx! {
                p { class: "lede", "Loading…" }
            },
            Some(View::Home) => rsx! {
                home::Home { on_open_session: move |id| view.set(Some(View::Session(id))) }
            },
            Some(View::Session(id)) => rsx! {
                session::SessionWorkspace {
                    session_id: id,
                    on_open_design: move |design_id| view.set(Some(View::Design(design_id))),
                    on_open_deck: move |deck_id| view.set(Some(View::Deck(deck_id))),
                    on_open_document: move |document_id| view.set(Some(View::Document(document_id))),
                    on_open_social: move |social_id| view.set(Some(View::Social(social_id))),
                    on_open_print: move |print_id| view.set(Some(View::Print(print_id))),
                    on_open_mailing: move |mailing_id| view.set(Some(View::Mailing(mailing_id))),
                    on_open_campaign: move |campaign_id| view.set(Some(View::Campaign(campaign_id))),
                    on_open_artwork: move |artwork_id| view.set(Some(View::Artwork(artwork_id))),
                    on_home: move |_| view.set(Some(View::Home)),
                }
            },
            Some(View::Design(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    editor::Editor {
                        design_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Deck(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    deck_editor::DeckEditor {
                        deck_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Document(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    document_editor::DocumentEditor {
                        document_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Social(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    social_editor::SocialEditor {
                        social_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Print(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    print_editor::PrintEditor {
                        print_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Mailing(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    mailing_editor::MailingEditor {
                        mailing_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Campaign(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    campaign_editor::CampaignEditor {
                        campaign_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Artwork(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    artwork_editor::ArtworkEditor {
                        artwork_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_holds_the_pinwheel_path_at_the_size_and_colour_asked_for() {
        let svg = logo_svg(32, "#0E6E63");
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\""));
        assert!(svg.contains("height=\"32\""));
        assert!(svg.contains("viewBox=\"0 0 178 179\""));
        assert!(svg.contains("fill=\"#0E6E63\""));
        assert!(svg.contains(LOGO_PATH));
        assert!(svg.ends_with("</svg>"));
    }

    #[test]
    fn the_tab_icon_draws_the_brand_path() {
        assert!(
            include_str!("../../../assets/favicon.svg").contains(LOGO_PATH),
            "assets/favicon.svg draws an old path: rebuild it and \
             assets/favicon.ico from LOGO_PATH"
        );
    }

    #[test]
    fn the_page_links_the_tab_icon_files() {
        let template = include_str!("../index.html");
        assert!(template.contains(r#"<link rel="icon" href="/favicon.ico" sizes="any">"#));
        assert!(template.contains(r#"type="image/svg+xml" href="/favicon.svg""#));
    }
}
