# Agent Implementation Guide

## Read first

Before making changes, inspect existing session, chat, runtime, artifact, persistence, and UI code. Reuse existing validation and storage boundaries where possible. Do not replace a working runtime abstraction merely to add this feature.

## Architecture rules

### State is authoritative

Use a persisted state machine:

```ts
type DesignWorkflowState =
  | "intake"
  | "clarifying"
  | "brief_ready"
  | "awaiting_approval"
  | "generating"
  | "reviewing"
  | "error";
```

Do not derive workflow state from natural-language text or artifact presence.

### Brief is authoritative

The structured approved brief, rather than raw chat history, is the canonical generation input. Each generation run records its brief ID/revision, runtime/provider/model, input, output references, timestamps, and result.

### Facts and assumptions stay separate

Persist and render user-confirmed facts, user answers, system assumptions, and unresolved questions in separate fields. Never present an assumption as confirmed user intent.

### Validate agent output

Validate structured output at the backend boundary. Reject or recover safely from unknown question types, more than three questions, duplicate IDs, invalid options, missing fields, and untrusted artifact/file paths. Prefer schemas over regex parsing.

## Prompt rules

### Briefing mode

The agent must:

- ask only material questions;
- ask at most three per turn;
- offer concise choices plus optional text;
- summarize facts and assumptions into a brief;
- never write files or generate artifacts.

### Generation mode

The agent must:

- treat the approved brief as authoritative;
- avoid overriding confirmed requirements;
- use recorded assumptions only where needed;
- return to clarification for blocking missing requirements;
- report changed files, validation, and material decisions.

## UX rules

- Do not turn the product into a long questionnaire.
- Always offer a reasonable skip/use-best-judgment path.
- Keep the brief editable and visible during generation.
- Show a concise rationale for questions when helpful.
- Preserve generation and revision history.
- A quick-generation escape hatch may exist for compatibility, but must not be the default.

## Error handling

Errors must be actionable and state-preserving. Invalid structured output should retain prior answers/brief and offer retry. Runtime failure should identify the runtime and setup action. Never expose credentials, authorization headers, private prompt data, or internal paths in browser responses/logs.

## Required tests

Cover workflow transitions, question count/schema validation, brief versioning, generation gating, generate-with-assumptions, persistence, briefing-mode no-file-write behavior, approved-brief generation context, critique revisions, malformed output, and runtime failure recovery.

Run relevant typecheck, unit, integration, and UI tests before completion.

## Completion checklist

- [ ] Persisted workflow state machine
- [ ] Validated structured question protocol
- [ ] Editable versioned brief
- [ ] Approval-gated generation
- [ ] Separate briefing and generation prompts
- [ ] Distinct facts/assumptions/open questions UI
- [ ] Revision flow
- [ ] Coverage for success and failure paths
