# Implement a Claude-Design-like briefing harness

Build a design-agent harness that prioritizes **brief discovery before artifact generation**.

Do not implement a prompt-to-immediate-artifact flow. The required product loop is:

> Discover → clarify → summarize/confirm brief → generate → critique → iterate

The agent must not generate an artifact until the brief is sufficiently complete or the user explicitly selects **Generate with assumptions**.

## Product goal

Given an underspecified request such as “Design a landing page for my finance app,” identify the few missing facts that materially affect the design, ask them in a structured form, retain answers, create a concise design brief, request confirmation, and then generate.

The product should feel like a design partner, not a code-generation form.

## Required behavior

### 1. Persisted workflow state machine

Implement these explicit states:

- `intake` — initial request received
- `clarifying` — collecting material missing information
- `brief_ready` — brief is ready for review
- `awaiting_approval` — user can approve, edit, or generate with assumptions
- `generating` — agent creates or updates files/artifacts
- `reviewing` — artifact is complete and can be critiqued
- `error` — recoverable failure

Persist this state. Do not infer it from chat text or frontend-only booleans.

### 2. Clarification policy

Ask only when an answer materially changes the design. Prioritize:

1. artifact type and target platform
2. audience and primary user goal
3. primary action/conversion goal
4. required content, features, and constraints
5. brand assets, visual direction, accessibility requirements
6. technical constraints when relevant

Rules:

- Ask at most three questions per turn.
- Prefer choices plus an `Other` text option.
- Include `Use your best judgment` / `Skip`.
- Keep user-confirmed facts, assumptions, and open questions separate.
- If the prompt is sufficiently specific, skip questions and draft a brief.
- Never silently invent material brand, audience, or conversion requirements.

### 3. Structured question protocol

Use typed and validated structured data, not regex parsing of prose:

```ts
type BriefQuestion = {
  id: string;
  label: string;
  rationale?: string;
  kind: "single_select" | "multi_select" | "short_text" | "long_text";
  required: boolean;
  options?: Array<{ value: string; label: string }>;
  allowOther?: boolean;
};

type BriefQuestionSet = {
  title: string;
  message: string;
  questions: BriefQuestion[];
  canProceedWithAssumptions: boolean;
};
```

### 4. Canonical versioned design brief

Create a durable brief that records:

- request and answers
- confirmed facts, assumptions, and open questions
- target artifact/platform
- audience and user problem
- primary job-to-be-done and success criterion
- information architecture
- required screens/sections and content
- visual direction and brand assets
- accessibility/technical constraints
- generation instructions
- revision history

The approved brief is the canonical input to generation. Edits must create a new revision.

### 5. UI

Build a conversational UI, not a blocking form wizard. Include:

- chat/intake area
- structured question cards
- answer summary
- editable brief panel
- `Approve brief and generate` and `Generate with assumptions` actions
- distinct facts, assumptions, and open questions
- meaningful generation progress
- critique controls for visual direction, structure, accessibility, content, and free-form feedback

### 6. Separate agent modes

**Briefing mode** may analyze, ask questions, and update the brief. It must not write files or emit artifacts.

**Generation mode** receives the approved brief, creates artifacts/files, validates output, and reports decisions. It must treat the brief as authoritative; blocking missing details return to clarification rather than being silently reinterpreted.

### 7. Persistence and resumability

Persist sessions, brief revisions, question sets, answers, generation runs, and artifact references. Reloading the browser or restarting the server must preserve active state and history.

### 8. Tests

Add unit, integration, and UI coverage for:

- vague request enters `clarifying`
- specific request can enter `brief_ready`
- no more than three questions per turn
- skipped answers become explicit assumptions
- generation is gated by approval except explicit assumptions flow
- approved brief is generation context
- state survives refresh
- brief edits version correctly
- malformed structured output fails safely
- critique starts a revision flow

## Implementation order

1. Inspect the existing session, runtime, persistence, artifact, and UI boundaries.
2. Add domain models and state machine.
3. Add validated backend/API contracts and persistence.
4. Build question cards and brief UI.
5. Gate the existing generation path behind approval.
6. Add briefing and generation prompts/adapters.
7. Add tests and run the project validation suite.

## Non-goals

- Do not create a full Figma replacement.
- Do not ask a lengthy questionnaire.
- Do not require every answer.
- Do not expose credentials, private prompts, or internal paths.

## Definition of done

A user can submit a vague request, answer a few material questions, approve an editable brief, receive an artifact, and request a focused revision without losing context.
