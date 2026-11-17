---
name: completion-audit
description: An adversarial pass that tries to prove a ticket or a group of tickets is not done, run before calling it done, opening its pull request, or using the word "done". Hunts for work declared complete on structure, such as code that exists, tests that pass and documents that say so, instead of on observed behaviour. Use before any completion claim, when re-reviewing work previously called done, or when asked whether something is really complete.
---

# completion-audit

Work was marked complete with checklists unticked, audit tables still showing
their starting state, and no proof that it worked anywhere real. In another
case a whole observability stack shipped structurally complete, with its tests
green, and **blank**: no metric ever reached the store. Both are the same
failure. "Done" was declared on structure, meaning the code exists, the tests
pass and the documents say so, instead of being proven on live behaviour, and
nothing adversarial checked before the claim.

This skill is that check. Its job is **not** to confirm the work is done. Its
job is to try to prove it is **not**, and to clear it only when it cannot.

The conventions in [the skills README](../README.md) apply.

## When to use

This is mandatory before any completion language:

- before marking a ticket or a group of tickets complete, opening its pull
  request, or saying "done";
- when re-reviewing work that was called done earlier;
- when asked whether something is actually complete;
- after the checks on whether each story is built and whether it is tested,
  as the last pass over the whole deliverable. Those are per story. This is
  the sweep across everything.

## Posture

Default to incomplete. For every sign of "done", whether a ticked box, a
passing cell, a merged child ticket or a green gate, ask **"what would make
this a lie?"**, and go and look. A claim you cannot back with a cited artefact
is treated as not done, not as done until disproved.

## The sweep

Walk the deliverable and hunt each of these in turn. Together they are the
bar for done.

1. **Marked done, never driven live.** For every deliverable, whether a story
   or a pipeline, integration or observability piece, demand the **real signal
   observed at the far end**: a user completed the journey through the real
   interface, or a metric was seen to land in the store and render, not merely
   that the code which emits it exists. If the only proof is a reading of the
   code, a unit test or a synthetic-data gate, it is not done. Drive it on the
   real system first, locally where that is cheap, and only then on a shared
   or deployed environment.

2. **A gate that checks structure standing in for proof.** For each gate
   cited as evidence, ask what data it ran on. A gate that seeds its own
   data, checks that at least one series exists, checks that two lists agree,
   or checks that tests exist or a route is wired, shows the artefact is well
   formed. It does not show it works with real data. Require at least one
   gate that exercised the real path with real data. Where a check uses
   synthetic data, the report says so and names the companion that does not.

3. **Claims with no evidence.** Every ticked box and every passing cell cites
   how it was proven: a named test you verified exists, a command and its
   captured output, something observed live, or a reference to the change. A
   bare tick is a claim, and a claim is treated as not done. Judge the
   quality of the evidence, not only its presence.

4. **References that point at nothing.** Every ticket or pull request number
   cited exists and is about what it is cited for. A deferral hidden behind a
   number that goes nowhere is unfinished work dressed as tracked. Deferrals
   become a real ticket.

5. **Tracking that is stale on shipped work.** A coverage table still showing
   "baseline, before implementation" on code that has merged, an unticked
   checklist on work that has shipped, a completeness table with no file and
   line and no live-drive result: each is itself a completion defect.
   Regenerate it to match reality, or the work is not done.

6. **Named scope quietly dropped.** Compare the ticket's summary and its list
   of stories with what shipped. A story reclassified "not applicable, see
   elsewhere", something the ticket claims but did not deliver, a phase that
   became a no-op: the summary is edited to match what was done, or the
   feature is built.

7. **A kind of review deferred to one that never ran.** A design review
   deliberately leaves standards, idiom, schema and contract shape to the
   reviews that own them, and a review that says "that is covered elsewhere"
   is only true if the other review ran. Work out which components the change
   touches and which review lenses that requires, and demand a recorded verdict
   for each, pinned to the commit it covered. A missing one is **not done**.
   In one audit of a campaign, nearly every ticket had a design review and one
   had any review of the other kind, across hundreds of files, because the
   mechanical checks were green throughout and nobody noticed that none of the
   judgement had happened.

8. **Findings left dangling, and logs not read.** Every blocking finding is
   fixed. Every other finding is either fixed or moved to a tracked ticket
   with a written reason: advisory does not mean ignored. During the live
   drive, the logs of every component were read and show no unexplained
   errors, failures or stack traces attributable to the change. A green suite
   can sit on top of a log full of swallowed errors.

9. **Integration, not just isolation.** For a group of tickets: the
   whole-repository verification (`manifest:gates.*.run`) is green on the
   integrated branch **after every merge**, and the system-level review
   covers the cumulative change. A child that is green on its own can break
   the whole.

10. **Attestations, and residue that nobody owns.** An absolute statement
    cited as evidence, "no further orphans", "all clean", "the runner
    completes", is a claim to break. Demand the red run of a planted
    violation for any guard, and a caller for any runner. A guard once
    certified clean while blind to the one violation that existed, and a
    runner was shown working while nothing ever called it. Separately, search
    the close-out notes for phrases such as "remains", "follow-up" or "not
    filed" that have no ticket behind them. Undischarged residue is unfinished
    work dressed as done, the same as a reference that goes nowhere.

Where the system has layers of behaviour, such as its inputs, authentication,
persistence, background work, calls to other services, storage, caching,
telemetry, performance and cost, each one that applies is validated, or marked
not applicable with a one-line reason. Events and metrics are asserted: the
counter fires, and the record is written with the right content. A layer is
never silently skipped.

## Output

A verdict, not a rubber stamp.

- **Pass**, only when every class above is clear, each with a cited artefact.
  Say what you drove live and what you observed.
- **Fail**, with a numbered list. For each gap: which item of the sweep it
  fails, what is missing, and the concrete artefact or drive that would clear
  it. A fail blocks completion language, the pull request, and the next phase.
- Prefer an honest fail to a pass that hides a deferral. "I could not drive X
  live, so it is at most partial" is a valid and required answer.

## Scale

For a group of tickets, give one subagent to each, if you have subagents, for
the first six items, and judge integration (items 9 and 10) yourself. Spot-
check what each one cites: search for a sample of the test names it gives and
run one gate again. Do not trust the citations blind.
