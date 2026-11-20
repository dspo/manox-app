---
name: todos
description: Use when the user asks for a plan or todo list for this conversation, or when a task needs three or more real steps. Covers creating the shared TODO plan, running it in order, recording completions, and reconciling the user's own edits.
---

# The shared TODO plan

Every conversation may carry one TODO plan, stored as a file the user can
watch and edit from the panel while you work. You read and change it only
through the `todo_*` tools. The plan is not your scratchpad: the user sees
every item, adds their own, and deletes yours. Never re-create an item the
user removed; treat their edit as the newest instruction.

## When to create a plan

- The user asks for a plan, a todo list, or "track this" — create one.
- A task needs three or more substantive steps and you already understand
  it — create one. One or two steps, or a task whose shape is still
  unclear, does not earn a plan; do the work or ask first.
- Never create bookkeeping items ("make a plan", "read the files"). A plan
  holds work the user asked for, not your own process.

## Shape

- One group per user request, titled by its outcome ("Exporter ships",
  never "Step 1"). Three to seven steps fit one group; past ten, split the
  request into more groups.
- Loose drafts (`todo_add` without a group) are sketches shared before a
  request takes shape. When one hardens into a request, promote it —
  `todo_promote` is the only way from the lane into a group. `todo_write`
  restructures groups; it never touches the lane.
- `todo_write` is a merge by id, not an overwrite: known ids keep their
  status, unseen ids become pending steps, and grouped ids you omit are
  deleted. List every group you want to keep.

## Running it

- One step in progress at a time, in order. Setting a second one demotes
  the first — that is the plan telling you to finish or explicitly park
  what you started.
- Blocked? Write what blocks you into the item's note, say so, and move to
  the next independent step. Do not spin on a blocked step.
- Re-read `todo_list` before choosing the next step, after every user
  message, and before you report — the user may have added, ticked, or
  deleted something since you last looked.
- A step the user reopens is work to do again, with whatever they added.
  A fix request reopens the same item rather than adding a twin.

## Completing a step

Mark it done and, for a step that changed code, record in the same patch:

- `summary` — what actually happened, one line;
- `verification` — the check you ran to believe it (a command, a test
  run). Nothing you did not run; "should work" is not a verification;
- `commitSubject` — a usable commit title, when the step produced one.

A done item stays in the plan as the record of what shipped. The plan's
history is the user's receipt — keep it readable.
