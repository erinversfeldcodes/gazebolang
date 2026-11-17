---
name: verify-and-followup
description: Run the repository's verification for what changed, report each result with evidence, and turn anything left over into a tracked follow-up. Use after finishing a change, before saying it is done, or when asked to verify, run the gates or close out.
---

# verify-and-followup

Confirm, execute, verify, then plan the next step. Verification is not "the
tests pass". It is evidence, plus a tracked home for anything left open.

The conventions in [the skills README](../README.md) apply.

## Steps

1. **Pick the right verification for what changed.** Neither more nor less
   than the change needs.
   - The tests and checks of each component the change touches are in the
     manifest (`manifest:components.*.test` and `manifest:components.*.lint`),
     and the components a change touches are those whose `paths` match its
     files.
   - Whole-repository gates are in the manifest too (`manifest:gates.*.run`),
     each with the changes it applies to (`manifest:gates.*.when_paths`) and
     how long it may run. A gate that applies to your change is one you run.
   - For a single suite, run it on its own first for the fast answer, and then
     the wider verification before declaring the work done.
   - Run long commands in the background and read their output when they
     finish, so one slow gate does not hide the result of the others.

2. **Report with evidence, never assertion.** Cite the number and the gate:
   "412 tests, 0 failures", not "the tests pass". If something failed, quote
   the failing output and say what kind of failure it is: a real regression,
   an environmental flake, or stale tooling. **Never dismiss a failure as "not
   ours".** Reproduce it or explain it. If a check was not run, say that it
   was not, and why.

3. **Close the loop on what is left.** For every gap, weakened test or finding
   that is larger than the change in hand:
   - If it is inside this change's scope and small, fix it now and run the
     verification again.
   - If it is outside scope, open a follow-up ticket with how to reproduce it,
     the root cause if you know it, and a proposed fix. If the gap is in test
     coverage, say which behaviour is unprotected and by what kind of test it
     should be. Name the new ticket in your report.

   A new discovery is a new ticket. It is not a larger version of this one.

4. **Report once, briefly.** Each gate with its result and number, the
   follow-up tickets you opened, and one line: done, or not done.

## Notes

- This is the light loop for a single change. A larger piece of work with
  several phases has its own sequence of gates, and that sequence is the one
  to follow.
- A failure you leave unexplained is a failure someone else will meet.
