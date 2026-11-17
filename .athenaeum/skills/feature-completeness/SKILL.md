---
name: feature-completeness
description: Before writing tests for a story, or a piece of work whose job is to validate stories, prove that each story's happy path is actually built end to end in the running system, not only that tests are missing. Also use for any pipeline, integration or observability deliverable, whose failure mode is being fully coded and delivering nothing. Answers "is it built?", which must be settled before "is it tested?".
---

# feature-completeness

A piece of work set out to validate six stories, went green, and yet one of
them had never been built: its core behaviour had been quietly deferred
elsewhere, and the checks went green by reclassifying its cells as not
applicable. The deferred feature, built later with no design pass, then caused
a run of follow-up fixes.

This skill closes that hole. It asks of each story: is it built, end to end,
in the running system? That question has to be answered before the question of
whether it is tested, because a green result should mean every named story is
built and correct, not that everything in a test-only charter is covered.

The conventions in [the skills README](../README.md) apply.

## When to use

- Any piece of work that validates stories, covers them with tests or hardens
  their tests, at planning time and before any test is written. Stories are
  wherever the manifest says (`manifest:docs.stories`).
- Any integration, pipeline or observability deliverable, whether or not it
  has a story. These fail in a particular way: they can be fully coded, their
  tests green, and still deliver nothing. For them, "built" means a real
  signal travels the whole path and is observed at the far end: an event
  emitted, stored, read back and shown, not that the code which emits it
  exists. One observability stack shipped structurally complete and blank,
  because no sample ever reached the store, and nothing that read code or ran
  on synthetic data could see it.
- When picking up work that claims to deliver something you have not seen
  work end to end with a real signal.
- When a test-coverage cell is about to be marked "not applicable" and you are
  unsure whether the test is missing or the feature is.

## The check, for each named story

1. **Trace the happy path through the real code.** Name every hop, with file
   and line: the entry point is wired, the logic returns real data and not a
   stub or a default value, every side effect the story implies actually
   fires, the result reaches the user's surface, and that surface can be
   reached by navigating to it. A story is not built because the back end
   exists. It is built when a real user can complete the journey it describes.

2. **Drive it live.** Reading code is necessary and not sufficient. Run the
   real system and complete the story the way a user reaches it: through the
   same entry point, with real dependencies, not a shortcut that skips the
   steps that matter. Three of the worst defects in one piece of work, logout
   that did not revoke the session, onboarding that did not start on a fresh
   login, a role that did not carry through, all passed a reading of the code
   and fell only to a live run. How to stand the system up is the
   repository's own business, and may be a gate it declares
   (`manifest:gates.*.run`). If you cannot drive it, say so: a claim of
   "implemented" with no live observation is at most partial.

3. **Give a verdict.**
   - **Implemented:** the happy path exists end to end and was observed
     working live.
   - **Partial:** it exists but is incomplete. A hop is stubbed, it is wired
     on only some surfaces, a promised side effect is missing, or it works
     only read-only. List exactly which hops are missing.
   - **Missing:** the core behaviour is not built.

## Anything less than implemented is a blocking finding

A partial or missing happy path on a named story is not a test-coverage
cell. It has exactly two legitimate resolutions, and never a quiet "not
applicable, see elsewhere":

- **Build it in scope.** Add the implementation to this piece of work. For
  anything non-trivial, such as sessions and authentication, payments, or
  anything with security or several surfaces, do a short design pass first and
  record it, and do not leave it as a stretch item at the end. The follow-up
  fixes that followed the deferred feature above are what a skipped design
  pass costs: the session model, with its absolute limits, rotation, reuse
  detection and races between tabs and in-flight requests, was discovered one
  fix at a time instead of designed once.
- **De-scope it.** Remove the story from this piece of work's summary and its
  list of stories, and open a new ticket for it. Editing the summary is
  mandatory: a ticket may not claim a story it is not delivering.

**Forbidden:** writing a test against a missing or stubbed feature, which
either fails for ever or passes vacuously against the stub, and both are worse
than an honest "missing". Also forbidden is reaching green by reclassifying a
named story's core behaviour as not applicable. "Not applicable" is only ever
for a layer that genuinely does not apply to a story that is built, such as
"logout emits no events, by design".

## Output

- A table for the ticket, one row per story, placed before any list of test
  coverage, because completeness comes before coverage:

  | Story | Happy-path hops (file:line) | Live-drive result | Verdict | Resolution |
  |---|---|---|---|---|

- A line added to the definition of done: every named story is implemented,
  each happy path built end to end and observed working on a live system, and
  any partial or missing story is either built in scope or de-scoped with its
  summary edited and a new ticket opened. No named story reaches green by
  being marked not applicable.
- A short report: the verdict for each story, any blocking findings, and the
  resolution you recommend for each. Not the whole trace.

## Scale

For many tickets, give one subagent to each, if you have subagents. Each
traces and drives its own stories. Then spot-check a sample of the file and
line references each one cited.
