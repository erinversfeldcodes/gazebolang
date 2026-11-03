import { test, expect } from "./fixtures";
import { uniqueWord, withFormResponse } from "./helpers";

test("creating a card shows it in the shared card list", async ({ page }) => {
  const word = uniqueWord("cards-spec");

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByLabel("Tags (comma separated)").fill("e2e");
  await page.getByRole("button", { name: "Add" }).click();

  await expect(page).toHaveURL("/");
  // The word is behind each card's "Reveal" control (see app.rs's
  // CardListPage — only the prompt/media shows up front) — hasText checks
  // textContent, not visibility, so this still confirms the card landed.
  await expect(page.locator("li", { hasText: word })).toHaveCount(1);
});

test("filtering by tag narrows the card list to matching cards", async ({ page }) => {
  const word = uniqueWord("cards-spec-tag");
  const tag = uniqueWord("e2e-tag");

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByLabel("Tags (comma separated)").fill(tag);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.getByLabel("Filter by tag").selectOption(tag);
  await expect(page.locator("li", { hasText: word })).toHaveCount(1);
});

test("editing a card updates its word and is reflected in the list", async ({ page }) => {
  const word = uniqueWord("cards-spec-edit");
  const updated = `${word}-updated`;

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  await page.getByLabel("Target-language word/phrase").fill(updated);
  // exact: true — the edit page's always-visible dictionary-details
  // section has its own "Save dictionary details" button, which "Save"
  // would otherwise substring-match too.
  //
  // This save lands on the *same* /cards/:id URL we're already on, so
  // `toHaveURL` below was never actually proof the save round-tripped — it
  // was already true before the click. Nothing else forced a wait for it
  // either, so the `page.goto("/")` a few lines down used to fire while the
  // POST was still in flight and cancel it outright (status -1 in a CI
  // trace) — the edit was silently lost, and the list page correctly
  // showed the pre-edit word. withFormResponse closes that race by waiting
  // for the POST's own response first, same fix withServerResponse applies
  // to /api/ server actions elsewhere.
  await withFormResponse(page, /^\/cards\/[0-9a-f-]+$/, () =>
    page.getByRole("button", { name: "Save", exact: true }).click(),
  );

  await expect(page).toHaveURL(/\/cards\/[0-9a-f-]+$/);
  await expect(page.getByLabel("Target-language word/phrase")).toHaveValue(updated);

  await page.goto("/");
  await expect(page.locator("li", { hasText: updated })).toHaveCount(1);
});

test("deleting a card removes it from the list", async ({ page }) => {
  const word = uniqueWord("cards-spec-delete");

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  // Delete requires an arming click before the confirming one — see
  // app.rs's confirm_button.
  await page.getByRole("button", { name: "Delete card" }).click();
  await page.getByRole("button", { name: "Confirm delete?" }).click();

  await expect(page).toHaveURL("/");
  // Not just "not visible" — the word is hidden behind Reveal for every
  // card now, visible or not, so absence has to be checked by whether the
  // card's <li> exists in the DOM at all.
  await expect(page.locator("li", { hasText: word })).toHaveCount(0);
});

test("submitting an invalid YouTube link on the new-card form shows an inline error", async ({
  page,
}) => {
  const word = uniqueWord("cards-spec-invalid-youtube");

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByLabel("Or link a YouTube video").fill("https://example.com/not-a-youtube-link");
  await page.getByRole("button", { name: "Add" }).click();

  // This form submits via leptos_router's <Form> (AJAX, redirect-follow via
  // client-side navigate), not a hard page reload — this is the regression
  // guard for that specifically: the error must still land back on /new
  // with the banner rendered, not silently vanish (see form_error_banner's
  // doc comment in app.rs for why it has to read reactively now).
  await expect(page).toHaveURL(/\/new\?error=/);
  await expect(page.getByText("Error:")).toBeVisible();
  await expect(page.locator("li", { hasText: word })).toHaveCount(0);
});

test("saving an edited card with an invalid YouTube link shows an inline error without losing the page", async ({
  page,
}) => {
  const word = uniqueWord("cards-spec-edit-invalid-youtube");

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  // A syntactically well-formed URL that still isn't a YouTube link — the
  // field is `type="url"`, so anything that isn't even URL-shaped (e.g.
  // "not a link at all") gets blocked by the browser's own native
  // constraint validation before the click ever reaches our submit
  // handler, which would test nothing about the server-side rejection
  // this is actually meant to guard.
  await page.getByLabel("Replace YouTube link (optional)").fill("https://example.com/not-a-youtube-link");
  await page.getByRole("button", { name: "Save", exact: true }).click();

  await expect(page).toHaveURL(/\/cards\/[0-9a-f-]+\?error=/);
  await expect(page.getByText("Error:")).toBeVisible();
  // The regression this guards: EditCardPage's `card` Resource is keyed
  // only on the card id, which doesn't change on this redirect, so
  // without `card_refresh` wired to <Form>'s own `version` prop it would
  // never re-fetch — confirmed live this used to leave the page stuck
  // showing stale data after a save.
  await expect(page.getByLabel("Target-language word/phrase")).toHaveValue(word);
});

test("an anonymous visitor sees a card read-only, with no edit form", async ({ page, browser }) => {
  const word = uniqueWord("cards-spec-anon");

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");
  const cardUrl = await page
    .locator("li", { hasText: word })
    .getByRole("link", { name: "Edit" })
    .getAttribute("href");
  if (!cardUrl) throw new Error(`couldn't find the card link for "${word}"`);

  // browser.newContext() doesn't inherit the project's `baseURL` the way
  // the `page` fixture does, so a relative href like cardUrl would fail to
  // resolve — build an absolute URL against the authenticated page's own
  // origin instead.
  const absoluteCardUrl = new URL(cardUrl, page.url()).toString();

  // The "chromium" project configures a default `storageState` (the signed-in
  // e2e user) for the `page`/`context` fixtures, and `browser.newContext()`
  // picks that ambient state up too unless told otherwise — so without this,
  // "anonPage" is silently signed in. Force a blank slate explicitly.
  const anonContext = await browser.newContext({ storageState: { cookies: [], origins: [] } });
  const anonPage = await anonContext.newPage();
  try {
    await anonPage.goto(absoluteCardUrl);
    // Browsing a card to self-test is open to anyone, in the same
    // reveal-then-show shape as the review queue — but there must be no
    // way to grade it, since grading is what writes to the Anki-derived
    // SRS state and that requires an account.
    await expect(anonPage.getByText(word, { exact: true })).not.toBeVisible();
    await expect(anonPage.locator(".review-grades")).toHaveCount(0);
    await anonPage.getByText("Reveal").click();
    await expect(anonPage.getByText(word, { exact: true })).toBeVisible();
    await expect(anonPage.getByLabel("Target-language word/phrase")).not.toBeVisible();
    await expect(anonPage.getByRole("button", { name: "Delete card" })).not.toBeVisible();
    // Scoped to the nav: the page also has a second "Sign in" link in its
    // own "Sign in to edit this card" prompt, so the bare role query matches
    // both and trips Playwright's strict mode.
    await expect(
      anonPage.getByRole("navigation").getByRole("link", { name: "Sign in" }),
    ).toBeVisible();
  } finally {
    await anonContext.close();
  }
});
