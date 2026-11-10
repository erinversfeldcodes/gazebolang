import { Page, expect } from "@playwright/test";
import { Client } from "pg";
import * as crypto from "crypto";

// Every environment that actually sends mail (preview, and prod) uses
// Resend's sandbox sender (onboarding@resend.dev) rather than a verified
// custom domain. That sandbox rejects any recipient other than the
// account owner's own exact address — no aliases, no other domains, not
// even "@example.com" — so any e2e test that triggers a real email send
// (password-reset requests, now that magic-link is gone) must target this
// literal address rather than a unique per-run one.
export const RESEND_ALLOWED_EMAIL = "erinversfeld@gmail.com";

// The fixed password for RESEND_ALLOWED_EMAIL's account. auth.setup.ts
// signs up with it on a fresh database and logs in with it otherwise
// (password auth rejects a repeat signup for an email that already
// exists), so every other spec file's storageState depends on this exact
// value staying in sync with whatever auth.setup.ts actually set.
export const TEST_PASSWORD = "correct-horse-battery-staple";

export function uniqueWord(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
}

// A Leptos server action's fetch is dispatched asynchronously from its
// triggering DOM event (a checkbox's on:change, a form's submit), so the
// checkbox/input's own post-click state is just the browser's optimistic
// local echo — not confirmation the server received or committed anything.
// `waitForLoadState("networkidle")` is not a reliable stand-in for that
// confirmation either: if the fetch hasn't actually been dispatched yet
// when the idle check runs, it reports idle immediately. A real page
// navigation right after (page.goto, page.reload) then cancels that
// still-in-flight request outright — observed directly in CI traces as
// the request landing with status -1. Registering the expected response
// before triggering the action, and awaiting both together, closes that
// window for good instead of hoping a heuristic wait covers it.
export async function withServerResponse<T>(
  page: Page,
  fnName: string,
  trigger: () => Promise<T>,
): Promise<T> {
  const [, result] = await Promise.all([
    page.waitForResponse((res) => res.url().includes(`/api/${fnName}`) && res.ok()),
    trigger(),
  ]);
  return result;
}

// Same race as withServerResponse, for a plain <Form> that posts straight to
// a hand-written axum route (e.g. CardEditForm's POST to /cards/:id) rather
// than a `/api/<fn>` server action — there's no fnName to match on, only the
// request path.
export async function withFormResponse<T>(
  page: Page,
  pathPattern: RegExp,
  trigger: () => Promise<T>,
): Promise<T> {
  const [, result] = await Promise.all([
    page.waitForResponse(
      (res) =>
        res.request().method() === "POST" && pathPattern.test(new URL(res.url()).pathname),
    ),
    trigger(),
  ]);
  return result;
}

// Creates a card and a deck containing only that card. Scoping a spec's
// review to its own deck (rather than the shared pool's review queue) is
// what makes it safe under parallel execution: specs running concurrently
// no longer compete over which card the queue offers next, or risk one
// spec grading away another's target card while searching for its own.
export async function createCardInOwnDeck(
  page: Page,
  prefix: string,
): Promise<{ word: string; deckName: string }> {
  const word = uniqueWord(prefix);
  const deckName = uniqueWord(`${prefix}-deck`);

  await page.goto("/decks");
  await page.getByLabel("New deck name").fill(deckName);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("link", { name: deckName })).toBeVisible();

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  await withServerResponse(page, "add_card_to_deck", () => page.getByLabel(deckName).check());
  await expect(page.getByLabel(deckName)).toBeChecked();

  return { word, deckName };
}

// Records a short clip via the given kind's RecordButton ("audio" or
// "video"), using the fake media device Chromium is launched with (see
// playwright.config.ts) — no real mic/camera involved, no human needed to
// grant a permission prompt. Waits for the recorder to actually report
// itself recording (confirms getUserMedia genuinely succeeded against the
// fake device, not just that the click landed), accumulates at least one
// second so MediaRecorder's periodic dataavailable actually fires at
// least once, then stops and waits for the resulting preview player.
// Located by which file input it sits next to, not by position — card
// type now gates which of audio/video (if either) actually render (see
// app.rs's MediaFields), so a card form doesn't always have both, or have
// them in the same order, the way `.recorder`'s nth(0)/nth(1) used to
// assume.
export async function recordClip(page: Page, kind: "audio" | "video"): Promise<void> {
  const recorder = page.locator(`label:has(input[accept="${kind}/*"]) + .recorder`);
  await recorder.getByRole("button", { name: "Record", exact: true }).click();
  await expect(recorder.getByRole("button", { name: "Stop recording" })).toBeVisible();
  await page.waitForTimeout(1500);
  await recorder.getByRole("button", { name: "Stop recording" }).click();
  await expect(recorder.locator(kind)).toBeVisible();
}

// Reveals and grades "Good" the one card in a deck created by
// createCardInOwnDeck, asserting it's actually that card.
export async function reviewOwnCard(page: Page, deckName: string, word: string): Promise<void> {
  await page.goto("/review");
  await page.getByLabel("Deck").selectOption(deckName);
  await expect(page.getByText("Reveal")).toBeVisible();
  await page.getByText("Reveal").click();
  await expect(page.getByText(word, { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Good" }).click();
  await page.waitForLoadState("networkidle");
}

// Account used only as the `invited_by` of invites this helper mints — a
// stand-in for whatever real user would normally click "Send invite" in
// the app. Since signup is now invite-only, there's no open path left to
// create the very *first* account on a fresh database at all, so this
// bootstrap account has to be written straight into Postgres (password_hash
// left NULL — the same "account with no password set yet" shape the
// magic-link-era rows in auth.rs's own tests use) rather than created
// through any UI.
const BOOTSTRAP_INVITER_EMAIL = "e2e-bootstrap-inviter@example.com";

async function ensureBootstrapInviter(client: Client): Promise<string> {
  const existing = await client.query("SELECT id FROM users WHERE email = $1", [
    BOOTSTRAP_INVITER_EMAIL,
  ]);
  if (existing.rows.length > 0) {
    return existing.rows[0].id as string;
  }
  const inserted = await client.query(
    "INSERT INTO users (email, name) VALUES ($1, $2) RETURNING id",
    [BOOTSTRAP_INVITER_EMAIL, "E2E Bootstrap"],
  );
  return inserted.rows[0].id as string;
}

// Mints a usable invite token for `email` by writing straight into the
// `invites` table, the same precedent auth.setup.ts's old magic-link
// version and auth.spec.ts's password-reset test already set for reading
// an auth token straight out of Postgres rather than out of a real inbox.
// Unlike those two, this doesn't even start from a real email send: the
// app's own `create_invite` server fn is exercised directly by
// invite.spec.ts instead, so this helper only needs to produce a redeemable
// token, not prove delivery.
export async function createInviteToken(email: string): Promise<string> {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    const inviterId = await ensureBootstrapInviter(client);
    const token = crypto.randomBytes(32).toString("hex");
    await client.query(
      "INSERT INTO invites (token, email, invited_by, expires_at) " +
        "VALUES ($1, $2, $3, now() + interval '7 days')",
      [token, email.toLowerCase(), inviterId],
    );
    return token;
  } finally {
    await client.end();
  }
}

// Deletes an account created via sign_up_via_invite, and everything that
// references it, so a spec that creates a throwaway account doesn't
// permanently eat one of MAX_USERS's 4 slots in the shared preview
// database (which, unlike a fresh local db-up, is reused across every push
// to a PR rather than reset) — see auth.spec.ts's deleteTestUser for the
// same concern against the same cap.
export async function deleteInvitedTestUser(email: string): Promise<void> {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    const lowered = email.toLowerCase();
    await client.query(
      "DELETE FROM sessions WHERE user_id = (SELECT id FROM users WHERE email = $1)",
      [lowered],
    );
    await client.query("DELETE FROM invites WHERE email = $1", [lowered]);
    await client.query("DELETE FROM users WHERE email = $1", [lowered]);
  } finally {
    await client.end();
  }
}
