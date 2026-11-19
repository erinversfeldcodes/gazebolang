# Working in this repository (agents and people)

## A red check blocks the change

A red required check blocks the change. None of these clears it, for an agent
or for a person:

- "The change did not touch the failing test, or the code under it." A change
  can fail a spec it never touched (shared data, ordering, a dependency, the
  deploy), and a spec can fail in a run of a change that could not have caused
  it. Neither is a finding about this run.
- "It passed when re-run." That says the failure is intermittent, not why. A
  re-run to green is not a cause, and neither is a higher retry count or a
  longer timeout.
- "It has failed before." Only a record of the same failure
  ([e2e/FLAKES.md](./e2e/FLAKES.md)) says that, and even then it is a fact for
  a person to weigh, not a clearance.

What clears a red check is its **cause**, shown from the failing run's own
trace or log (`gh run view <id> --log-failed`), and a fix for that cause. If
the cause cannot be found, the change is not merged.

An agent never merges past a red check and never uses `--admin` to do it. Only
a maintainer may merge past a recorded flake, by a comment on the pull request
that names the spec, the run, and where the cause is recorded. **An agent that
cannot clear a red check reports what it found and stops.**

Do not make a check non-required, skip a spec, raise the global timeout or
raise the retry count to get a green: each clears the red without a cause.
