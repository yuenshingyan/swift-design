# Brief-First Design Harness

A conversational design-agent harness that turns ambiguous requests into approved design briefs before generating artifacts.

## Why

Most design-generation tools optimize for:

```text
prompt → artifact
```

This harness optimizes for:

```text
request → clarify → brief → approve → generate → critique → revise
```

The purpose is not to delay output. It is to avoid poor alignment caused by guessing about audience, goals, content, brand direction, or platform.

## Core principles

- Ask only high-impact questions.
- Ask no more than three questions at once.
- Preserve answers, assumptions, and open questions separately.
- Treat the approved brief as the source of truth.
- Do not generate before approval unless the user explicitly selects **Generate with assumptions**.
- Make design decisions inspectable and revisable.

## Workflow

| State | Purpose |
|---|---|
| `intake` | Receive the initial request |
| `clarifying` | Ask focused design questions |
| `brief_ready` | Present the assembled brief |
| `awaiting_approval` | User approves, edits, or accepts assumptions |
| `generating` | Agent creates the artifact |
| `reviewing` | User critiques and requests revisions |
| `error` | Show a recoverable failure |

## Design brief

A brief records audience, user need, target artifact/platform, primary action, required content/functionality, information architecture, visual direction, brand assets, accessibility and technical constraints, assumptions, open questions, and revision history.

## Architecture

The harness may use a local CLI agent or a remote API, but workflow state, briefs, artifacts, and user decisions remain under application control.

There are two agent modes:

1. **Briefing mode** — asks, summarizes, and updates the brief; it cannot write artifacts.
2. **Generation mode** — creates the artifact from an approved brief.

See `AGENT.md` for implementation rules.
