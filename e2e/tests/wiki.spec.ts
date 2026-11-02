import { test, expect } from "./fixtures";
import { Page } from "@playwright/test";
import { uniqueWord } from "./helpers";

// The BlockNote island mounts asynchronously, well after data-hydrated="true"
// (fixtures.ts's goto patch only covers Leptos/WASM hydration) — it's a
// separately-injected script that then itself polls for its own mount point
// to exist (see wiki_editor.rs's mount()/wait_for_editor_script()/
// wait_for_mount_element(), each budgeted up to 5s), so a cold first mount on
// a page that's never loaded the editor script before can genuinely take a
// few seconds. Every test that edits a page waits for this first.
async function waitForEditorReady(page: Page): Promise<void> {
  await expect(page.locator(".wiki-editor-toolbar")).toBeVisible({ timeout: 15_000 });
}

async function createPage(page: Page, title: string): Promise<void> {
  await page.goto("/wiki");
  await page.getByLabel("New page").fill(title);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page).toHaveURL(/\/wiki\/[0-9a-f-]+/);
  await expect(page.getByRole("heading", { name: title })).toBeVisible();
}

// A fresh page's one empty paragraph — placed here rather than clicked via
// its placeholder text ("Enter text or type '/' for commands"), which is
// CSS generated content (.bn-block-content::before), not a real DOM text
// node getByText can match. .bn-block-content is BlockNote's own stable
// class for a block's editable content area.
async function clickIntoFirstBlock(page: Page): Promise<void> {
  await page.locator(".bn-block-content").first().click();
}

test("editing a page and clicking Done shows the saved content immediately, with no reload", async ({
  page,
}) => {
  // The actual regression this guards: the page Resource that feeds the read
  // view used to stop refetching after a save entirely (fixed to avoid a
  // worse bug — see app.rs's `page` Resource comment — but the fix initially
  // missed wiring up a refetch for the one moment that still needs one:
  // leaving edit mode). Confirmed live this exact flow showed stale
  // pre-edit content indefinitely until a full page reload.
  const title = uniqueWord("wiki-spec-persist");
  const bodyText = uniqueWord("wiki-spec-persist-body");
  await createPage(page, title);

  await page.getByRole("button", { name: "Edit" }).click();
  await waitForEditorReady(page);
  await clickIntoFirstBlock(page);
  await page.keyboard.type(bodyText);
  await page.getByRole("button", { name: "Done" }).click();

  // No page.reload() anywhere in this test — the point is that Done alone
  // is enough.
  await expect(page.getByRole("button", { name: "Edit" })).toBeVisible();
  await expect(page.getByText(bodyText)).toBeVisible();
});

test("a selected paragraph split into columns distributes its text across all of them", async ({
  page,
}) => {
  // The actual regression: columns only ever distributed whole *blocks*, so
  // selecting a single paragraph (the common case) and asking for N columns
  // put all of it in column one with the rest left empty, no matter how
  // long the paragraph was. Fixed by falling back to splitting the text
  // itself (word-boundary aware) whenever there aren't enough blocks to
  // fill every column.
  const title = uniqueWord("wiki-spec-columns");
  const sentence =
    "The quick brown fox jumps over the lazy dog while the sun sets slowly behind the distant mountains";
  await createPage(page, title);

  await page.getByRole("button", { name: "Edit" }).click();
  await waitForEditorReady(page);
  await clickIntoFirstBlock(page);
  await page.keyboard.type(sentence);
  // Triple-click rather than a Ctrl/Cmd+A keyboard shortcut: selects the
  // whole paragraph with a plain mouse action that behaves identically
  // regardless of the OS running the test.
  await page.getByText(sentence).click({ clickCount: 3 });

  await page.locator(".wiki-toolbar-columns button").click();
  await page.getByRole("button", { name: "3 columns" }).click();
  await page.getByRole("button", { name: "Done" }).click();

  const columns = page.locator(".wiki-columns .wiki-column");
  await expect(columns).toHaveCount(3);
  const texts = await columns.allTextContents();
  for (const columnText of texts) {
    expect(columnText.trim().length).toBeGreaterThan(0);
  }
  // Reassembling the columns (space-joined, since the split cuts at word
  // boundaries) reproduces the original sentence — confirms every word
  // made it across exactly once, not just that three non-empty divs exist.
  expect(texts.join(" ").replace(/\s+/g, " ").trim()).toBe(sentence);
});

test("Remove columns unwraps a layout back to plain stacked paragraphs", async ({ page }) => {
  const title = uniqueWord("wiki-spec-remove-columns");
  const sentence = "Removing columns should restore plain flowing text";
  await createPage(page, title);

  await page.getByRole("button", { name: "Edit" }).click();
  await waitForEditorReady(page);
  await clickIntoFirstBlock(page);
  await page.keyboard.type(sentence);
  await page.getByText(sentence).click({ clickCount: 3 });
  await page.locator(".wiki-toolbar-columns button").click();
  await page.getByRole("button", { name: "2 columns" }).click();

  // No selection needed for this one — just a cursor inside the layout
  // (clicking the first column's text places it there). .wiki-columns/
  // .wiki-column are wiki.rs's read-view classes, not available yet here
  // — BlockNote's own xl-multi-column extension renders the live layout
  // with its own classes while still in edit mode.
  await page.locator(".bn-block-content", { hasText: sentence.split(" ")[0] }).first().click();
  await page.locator(".wiki-toolbar-columns button").click();
  await page.getByRole("button", { name: "Remove columns" }).click();
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.locator(".wiki-columns")).toHaveCount(0);
  await expect(page.getByText(sentence.split(" ").join(" "))).toBeVisible();
});

test("two or more headings generate a table of contents linking to each one", async ({ page }) => {
  const title = uniqueWord("wiki-spec-toc");
  const heading1 = uniqueWord("wiki-spec-toc-first");
  const heading2 = uniqueWord("wiki-spec-toc-second");
  await createPage(page, title);

  await page.getByRole("button", { name: "Edit" }).click();
  await waitForEditorReady(page);

  await clickIntoFirstBlock(page);
  await page.locator(".wiki-toolbar-blocktype").selectOption({ label: "Heading 1" });
  await page.keyboard.type(heading1);
  await page.keyboard.press("Enter");
  await page.locator(".wiki-toolbar-blocktype").selectOption({ label: "Heading 1" });
  await page.keyboard.type(heading2);

  await page.getByRole("button", { name: "Done" }).click();

  const toc = page.locator("nav.wiki-toc");
  await expect(toc.getByText("Contents")).toBeVisible();
  const entries = toc.locator("li a");
  await expect(entries).toHaveCount(2);
  await expect(entries.nth(0)).toHaveText(heading1);
  await expect(entries.nth(1)).toHaveText(heading2);
});

test("typed content survives an autosave cycle instead of being lost or reset", async ({ page }) => {
  // The actual regression this guards: the page Resource that feeds the
  // live editor used to refetch on every autosave tick, tearing down and
  // rebuilding the entire BlockNote instance roughly every 6 seconds — not
  // just a cosmetic selection-loss bug, but one that could silently discard
  // keystrokes typed during the save round trip. Typing a second chunk
  // after an autosave has already fired, then confirming both chunks
  // survived to the read view, catches a regression here without needing
  // to inspect the live DOM selection directly.
  const title = uniqueWord("wiki-spec-autosave");
  const firstChunk = uniqueWord("wiki-spec-autosave-first");
  const secondChunk = uniqueWord("wiki-spec-autosave-second");
  await createPage(page, title);

  await page.getByRole("button", { name: "Edit" }).click();
  await waitForEditorReady(page);
  await clickIntoFirstBlock(page);
  await page.keyboard.type(firstChunk);

  // Just past app.rs's 6-second autosave interval, so the wait below is
  // guaranteed to straddle at least one real autosave tick.
  await page.waitForTimeout(7_000);
  await expect(page.getByText(firstChunk)).toBeVisible();

  await page.keyboard.type(" " + secondChunk);
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.getByText(firstChunk)).toBeVisible();
  await expect(page.getByText(secondChunk)).toBeVisible();
});
