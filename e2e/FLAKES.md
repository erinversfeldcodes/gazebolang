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

Not reproduced locally (the preview stack is not available here), so each
cause below is the best reading of the logs, not a proven one. The
before/after proof is the run history, below.

- **Slow backend, wait too short** — `invite:17`, `account:200`,
  `image-thumbnails:100`. Every failure is a form submit whose redirect had
  not happened yet: the page is still on the form's URL, with no error shown.
  Sign-up, login and password change each run Argon2 through a single-permit
  semaphore (`ARGON2_PERMITS` in `crates/web/src/auth.rs`, 10s checkout) on a
  shared-cpu machine, so with two workers they queue behind other specs'
  hashes; the card upload is decoded and thumbnailed before it redirects.
  Raising the timeout to 15s (already done for sign-up) did not stop it.
- **Slow backend, plus acting on a stale page** — `card-list-pagination:68`.
  The refetch after a filter or page-size change lands late on the preview
  stack (the sibling spec's comment says the same); this spec clicked Next
  without first waiting for the filtered pager, then asserted the result of
  the size change on the default 5s.
- Shared state (the one Neon branch) and ordering: no log shows either. Each
  spec seeds its own uniquely named rows (`uniqueWord`, tagged cards) and
  deletes them. `account.spec.ts` is serial by design. Not ruled out for
  `image-thumbnails`, which creates cards in the shared account.

## Fix

The specs now wait on the condition that matters, the server's response to the
request they triggered (`withServerCall` / `withFormResponse` in
`tests/helpers.ts`), and then assert as before. No global timeout or retry
count changed, and no assertion was weakened: each still checks the same
URL/text, which stays wrong if the behaviour breaks (a failed request fails
`response.ok()`).

## Failure rate

Before: of the last 68 completed `Preview` runs on non-dependabot branches
(fetched 2026-11-19), 31 ended in failure; these include deploy failures, and
the e2e share has not been separated out. The e2e specs above failed in 2 of
the 3 most recent runs inspected (44473605145, 44482710078).

After: **not yet measured.** Compare the same number of `Preview` runs after
this change merges (`gh run list --workflow Preview --json databaseId,conclusion`
then `--log-failed` for each red) and add the figures here. Until then the
flakiness is not shown to be fixed.
