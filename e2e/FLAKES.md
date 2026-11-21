# Intermittent failures on the preview end-to-end run

Evidence for a person to weigh. An entry here does **not** clear a red check
(see [AGENTS.md](../AGENTS.md)); only the failing run's own cause, and a fix
for it, does. Logs: `gh run view <run> --log-failed` (add `--attempt N`, or use
`gh api repos/<owner>/<repo>/actions/jobs/<job>/logs` for an earlier attempt).

Job: `Run E2E suite against preview` (workflow `Preview`). Config: 2 workers,
1 retry in CI, 5s default `expect` timeout.

## Records

| Spec | Failing assertion as logged | Failed | Passed |
|---|---|---|---|
| `card-list-pagination.spec.ts:68` changing the page size keeps your place… | `expect(locator).toBeVisible()` on `.card-pager` `Showing 1–13 of 13`, 5000ms, failed on retry too | run 44473605145 attempt 1 (job 135392836920) | same run, attempt 2 (126 passed) |
| `account.spec.ts:200` changing your password logs you out… | `toHaveURL("/")` 15000ms. First try: stuck at `/signup?token=…` inside `signUpFreshAccount` (line 51). Retry: stuck at `/login?reset=success` at the final login (line 230) | run 44473605145 attempt 1 | same run, attempt 2 |
| `image-thumbnails.spec.ts:100` review preloads the next card's image… | `toHaveURL("/")` 5000ms in `createCardWithImage` (line 38); URL still `/new` | run 44473605145 attempt 2 (flaky: passed on its retry) | attempt 1 |
| `invite.spec.ts:17` creating an invite and signing up through it… | `toHaveURL("/")` 15000ms at line 48; URL still `/signup?token=…`; no error rendered. Failed on its retry too | run 44482710078 (PR #60) | 44479789916 and earlier |

## Cause, per spec

**No cause is proven.** Not reproduced locally (no preview stack here). What the
failing logs do show, and what they do not:

- Shown (run 44482710078, `gh run view 44482710078 --log-failed`): `invite:17`
  fails on both tries at `invite.spec.ts:48`, `toHaveURL("/")`, 15000ms,
  `Received: .../signup?token=...`. The page never left the form and no error
  rendered. The same shape (URL unchanged after a submit, no error text)
  is logged for `account:200` and `image-thumbnails:100`.
- Not shown: whether the submit's request was in flight, queued, or never
  answered. Playwright's `toHaveURL` failure does not say, and no server-side
  timing was captured. The Argon2 single-permit queue (`ARGON2_PERMITS` in
  `crates/web/src/auth.rs`, 10s checkout) is a hypothesis for the three
  form-submit specs; "the response never arrives" fits the same log line.
  The logs for run 44473605145 are no longer retrievable with `gh run view
  --log-failed`, so its rows above come from the earlier investigation, not
  re-verified here.
- `card-list-pagination:68`: the failing assertion is a 5000ms wait for
  `Showing 1–13 of 13`; the sibling spec's comment says the refetch lands late
  on the preview stack. That supports "wait too short" only as a reading.
- Shared state (the one Neon branch) and order: no log shows either. Specs seed
  uniquely named rows and delete them; `account.spec.ts` is serial by design.

## Cause found, from run 44866377910 (PR #61, trace artifact `playwright-traces-pr-61`)

- `invite:17` failed on all three tries. The trace's `sign_up_via_invite`
  request is a form-encoded POST answered `302` with a `session` cookie and
  `Location: /signup?token=…`: a native form post, not the wasm fetch. The
  spec's `signupPage` comes from `browser.newContext()`, so its `goto` does not
  wait for hydration (the fixture only wraps the `page` fixture); clicking
  "Sign up" before hydration posts natively and lands back on the form. That is
  the same `URL still /signup?token=…` shape as the 15000ms failures above, so
  those were likely this, not a slow backend. Fix: `waitForHydration` on both
  new pages. Not yet shown to hold over several runs.
- `account:206` (first try) and `bulk-add:4` (both tries): the traces show the
  Fly proxy answering `502` (`log_in` after 2447ms; `POST /cards/bulk` after
  1697ms). The app was not answering. **Cause of the 502s not found**: the
  memory sampler's `samples.csv` is empty (ssh to the machine failed), so a
  restart/OOM is neither shown nor ruled out. No spec change addresses it.

## Mitigation (cause unproven, except invite)

The specs now wait on the server's response to the request they triggered
(`withServerCall` / `withFormResponse` in `tests/helpers.ts`), then assert as
before. No global timeout or retry count changed, and no assertion was
weakened: each still checks the same URL/text, and a failed request fails
`response.ok()`.

This turns the next failure into a discriminating one: if no response arrives
the test fails at its own timeout inside the helper (request never answered);
if a response arrives but is not ok, it fails on `response.ok()` with the
status; if it is ok but the URL/text is wrong, that is a behaviour fault. Until
a run shows which, the fix is a hypothesis, not a demonstrated cure. Next step
if a red recurs: read that failure's helper line and add server-side timing for
the request, rather than lengthening any wait.

Notes: `ARGON2_PERMITS` in `crates/web/src/auth.rs` is `Semaphore::new(1)`:
one Argon2 operation at a time (an earlier spec comment said 2; that was
wrong). Queueing behind it is a hypothesis for account:200 and
image-thumbnails, not measured. The assertions after each response keep their
original explicit 15s bound; the response wait has no timeout of its own, so a
hung request holds a test to its full test timeout, twice with the retry. No
post-response latency has been measured. The account:200 and 502 causes remain
open; only invite's cause (native form post before hydration) is shown by a
trace and fixed.

## Failure rate

Before: of the last 68 completed `Preview` runs on non-dependabot branches
(fetched 2026-11-19), 31 ended in failure, including deploy failures; the e2e
share is not separated out. The specs above failed in 2 of the 3 most recent
runs inspected (44473605145, 44482710078).

After: **not measured.** The only post-change `Preview` run so far is
44501511299 (this PR), which passed; one run shows nothing about an
intermittent failure. Compare the same 68 runs after merge
(`gh run list --workflow Preview --json databaseId,conclusion`, then
`--log-failed` for each red) and record the figures here.
