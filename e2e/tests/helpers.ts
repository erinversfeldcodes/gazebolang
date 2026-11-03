import { Page, expect } from "@playwright/test";

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
// Assumes the given page already has exactly one audio RecordButton and
// one video RecordButton, audio first — true of both the new-card and
// edit-card forms today.
export async function recordClip(page: Page, kind: "audio" | "video"): Promise<void> {
  const recorder = page.locator(".recorder").nth(kind === "audio" ? 0 : 1);
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
