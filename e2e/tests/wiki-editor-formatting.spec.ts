import { test, expect } from "./fixtures";
import { Page } from "@playwright/test";
import { uniqueWord } from "./helpers";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

// Covers the rest of the wiki editor's toolbar — one test per distinct
// feature (or per small group of mechanically-identical ones, like the five
// inline marks) rather than per bug, unlike wiki.spec.ts. Each assertion is
// checked against crates/web/src/wiki.rs's actual rendered HTML (confirmed
// against its own render_single_block/render_list/wrap_styles/style_attr
// and their own unit tests), not just "something changed in the editor" —
// the read view is the real product surface, and it has its own independent
// rendering logic that could silently drift from what the editor applies.
//
// Deliberately out of scope here: image/video/audio/file/emoji insertion.
// Those need either a real upload (file or recorded clip) or an external
// API (Pixabay, for image search) to exercise meaningfully, matching the
// existing media-cards.spec.ts/image-search.spec.ts — both of which are
// already environment-dependent (fake media device flags, live Pixabay
// access) rather than exercised here too.

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

async function clickIntoFirstBlock(page: Page): Promise<void> {
  await page.locator(".bn-block-content").first().click();
}

async function startEditingFreshPage(page: Page, titlePrefix: string): Promise<string> {
  const title = uniqueWord(titlePrefix);
  await createPage(page, title);
  await page.getByRole("button", { name: "Edit" }).click();
  await waitForEditorReady(page);
  await clickIntoFirstBlock(page);
  return title;
}

// uniqueWord() separates its timestamp/random suffix with hyphens, which
// breaks a plain double-click's native word-boundary selection (it stops at
// the hyphen, selecting only a fragment) — confirmed live this was a real
// test bug, not an app one, the one time it mattered here: selecting a
// whole uniqueWord() string by double-click rather than by triple-clicking
// its enclosing block.
function uniqueWordNoHyphens(prefix: string): string {
  return uniqueWord(prefix).replace(/-/g, "");
}

test("the five mark buttons wrap each selected word in the right inline tag", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-marks");

  // Confirmed live this is a real, separate bug (BlockNote/ProseMirror
  // level, not this app's own code) worth knowing about even though this
  // test routes around it rather than fixing it: applying a mark via the
  // toolbar's mousedown-based button, then immediately pressing Enter to
  // start a new paragraph, corrupts the paragraph split and silently
  // deletes the just-marked text — reproduced in isolation with no
  // selection/mark-button involvement at all working fine, and the exact
  // same toggleStyles call made via a keyboard shortcut (not a mousedown)
  // not triggering it either, narrowing it specifically to "toolbar mark
  // click immediately followed by Enter". Typing every word into its own
  // paragraph *first*, with no formatting applied yet, and only then
  // double-clicking back into each one to apply its mark — never pressing
  // Enter again afterward — sidesteps it entirely.
  for (const word of ["bold", "italic", "underline", "strike", "code"]) {
    await page.keyboard.type(word);
    await page.keyboard.press("Enter");
  }
  await page.keyboard.press("Backspace"); // the loop's trailing empty paragraph

  const applyMarkToWord = async (word: string, buttonTitle: string) => {
    await page.getByText(word, { exact: true }).dblclick();
    await page.locator(`button[title="${buttonTitle}"]`).click();
  };
  await applyMarkToWord("bold", "Bold");
  await applyMarkToWord("italic", "Italic");
  await applyMarkToWord("underline", "Underline");
  await applyMarkToWord("strike", "Strikethrough");
  await applyMarkToWord("code", "Code");

  await page.getByRole("button", { name: "Done" }).click();

  const article = page.locator(".wiki-article");
  await expect(article.locator("strong", { hasText: "bold" })).toBeVisible();
  await expect(article.locator("em", { hasText: "italic" })).toBeVisible();
  await expect(article.locator("u", { hasText: "underline" })).toBeVisible();
  await expect(article.locator("s", { hasText: "strike" })).toBeVisible();
  await expect(article.locator("code", { hasText: "code" })).toBeVisible();
});

test("text color and highlight color render as the matching inline style", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-color");
  const text = "colored text";
  await page.keyboard.type(text);
  await page.getByText(text).click({ clickCount: 3 });

  // One selection, both colors — not a fresh re-select between them: the
  // swatch buttons are mousedown-based specifically to preserve the active
  // selection (see Toolbar's own comment on this pattern), and re-selecting
  // via triple-click after the first color has already changed the DOM
  // (wrapped the text in its own styled span) hit the same
  // mousedown-then-DOM-mutation inconsistency documented on the marks test
  // above — confirmed live removing the redundant re-select here fixes it.
  await page.locator('button[title="Text color"]').click();
  await page.locator('.wiki-color-popover button[title="red"]').click();
  await page.locator('button[title="Highlight color"]').click();
  await page.locator('.wiki-color-popover button[title="blue"]').click();

  await page.getByRole("button", { name: "Done" }).click();

  // Exact hex values confirmed against wiki.rs's own
  // renders_text_color_and_background_color_as_an_inline_style test —
  // BlockNote's default palette, resolved server-side, not guessed at.
  await expect(
    page.locator('.wiki-article span[style="color:#e03e3e;background-color:#ddebf1"]', {
      hasText: text,
    }),
  ).toBeVisible();
});

test("center alignment renders as a text-align style on the block", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-align");
  const text = uniqueWord("wiki-fmt-align-text");
  await page.keyboard.type(text);
  // Block-level, not a mark — no selection needed, just a cursor in the
  // block (matches applyAlignment's own getTextCursorPosition()-based
  // implementation in main.tsx).
  await page.locator('button[title="Align center"]').click();
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.locator(`.wiki-article p[style="text-align:center"]`, { hasText: text })).toBeVisible();
});

test("Clear formatting removes a mark that was just applied", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-clear");
  const text = uniqueWord("wiki-fmt-clear-text");
  await page.keyboard.type(text);
  await page.getByText(text).click({ clickCount: 3 });
  await page.locator('button[title="Bold"]').click();
  await page.getByText(text).click({ clickCount: 3 });
  await page.locator('button[title="Clear formatting"]').click();
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.locator(".wiki-article")).toContainText(text);
  await expect(page.locator(".wiki-article strong", { hasText: text })).toHaveCount(0);
});

test("the Quote toolbar button renders a blockquote", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-quote");
  const text = uniqueWord("wiki-fmt-quote-text");
  await page.locator('button[title="Quote"]').click();
  await page.keyboard.type(text);
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.locator(".wiki-article blockquote", { hasText: text })).toBeVisible();
});

test("the Bullet list toolbar button renders a ul/li", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-bullets");
  const text = uniqueWord("wiki-fmt-bullets-text");
  await page.locator('button[title="Bullet list"]').click();
  await page.keyboard.type(text);
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.locator(".wiki-article ul li", { hasText: text })).toBeVisible();
});

test("the Numbered list toolbar button renders an ol/li", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-numbers");
  const text = uniqueWord("wiki-fmt-numbers-text");
  await page.locator('button[title="Numbered list"]').click();
  await page.keyboard.type(text);
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.locator(".wiki-article ol li", { hasText: text })).toBeVisible();
});

test("the Checklist toolbar button renders an unchecked checkbox by default", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-checklist");
  const text = uniqueWord("wiki-fmt-checklist-text");
  await page.locator('button[title="Checklist"]').click();
  await page.keyboard.type(text);
  await page.getByRole("button", { name: "Done" }).click();

  const item = page.locator(".wiki-article ul.wiki-checklist li", { hasText: text });
  await expect(item).toBeVisible();
  await expect(item.locator('input[type="checkbox"]')).not.toBeChecked();
});

test("the Toggle list toolbar button renders a ul.wiki-togglelist", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-togglelist");
  const text = uniqueWord("wiki-fmt-togglelist-text");
  await page.locator('button[title="Toggle list"]').click();
  await page.keyboard.type(text);
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.locator(".wiki-article ul.wiki-togglelist li", { hasText: text })).toBeVisible();
});

test("the Code block toolbar button renders a pre/code block", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-codeblock");
  const text = uniqueWord("wikiFmtCodeblockText");
  await page.locator('button[title="Code block"]').click();
  await page.keyboard.type(text);
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.locator(".wiki-article pre code", { hasText: text })).toBeVisible();
});

test("the Link popover turns a selected word into a real anchor", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-link");
  const word = uniqueWordNoHyphens("wikiFmtLinkWord");
  const url = "https://example.com/";
  await page.keyboard.type(word);
  await page.getByText(word, { exact: true }).dblclick();
  await page.locator('button[title="Link"]').click();
  await page.getByPlaceholder("https://…").fill(url);
  await page.getByRole("button", { name: "Apply" }).click();
  await page.getByRole("button", { name: "Done" }).click();

  const link = page.locator(".wiki-article a", { hasText: word });
  await expect(link).toBeVisible();
  await expect(link).toHaveAttribute("href", url);
});

test("the Table picker inserts a table with the chosen dimensions", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-table");
  await page.locator('button[title="Table"]').click();
  // The picker's cells are a flat 8x8 grid (TABLE_PICKER_SIZE in
  // main.tsx), row-major — index 9 (0-based) is row 2, col 2, each cell's
  // own onMouseDown inserting a table sized to that cell's row/col
  // directly, regardless of any prior hover.
  await page.locator(".wiki-table-picker-cell").nth(9).click();
  await page.getByRole("button", { name: "Done" }).click();

  const table = page.locator(".wiki-article table");
  await expect(table).toBeVisible();
  await expect(table.locator("tr")).toHaveCount(2);
  await expect(table.locator("tr").first().locator("td, th")).toHaveCount(2);
});

test("the Insert menu's Divider item renders an hr", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-divider");
  await page.locator(".wiki-toolbar-btn-insert").click();
  await page.getByRole("button", { name: "Divider" }).click();
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.locator(".wiki-article hr")).toBeVisible();
});

test("Import .md replaces the document with the file's parsed content", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-import");
  const heading = uniqueWord("wiki-fmt-import-heading");
  const tmpFile = path.join(os.tmpdir(), `${uniqueWord("wiki-fmt-import")}.md`);
  fs.writeFileSync(tmpFile, `# ${heading}\n`);

  await page.locator('input[type="file"][accept=".md,text/markdown"]').setInputFiles(tmpFile);
  await expect(page.getByRole("heading", { level: 1, name: heading })).toBeVisible();

  await page.getByRole("button", { name: "Done" }).click();
  await expect(page.locator(".wiki-article h1", { hasText: heading })).toBeVisible();

  fs.unlinkSync(tmpFile);
});

test("Ctrl/Cmd+B bolds the selection the same as the toolbar button", async ({ page }) => {
  await startEditingFreshPage(page, "wiki-fmt-shortcut");
  const word = uniqueWordNoHyphens("wikiFmtShortcutWord");
  await page.keyboard.type(word);
  // Triple-click, not a double-click word-select: confirmed live, repeatably,
  // that BlockNote's own native Mod+B handling (this app has no keyboard
  // handling of its own — see main.tsx's own history on this) only
  // reliably recognizes a selection ProseMirror itself built via triple-
  // click; a double-click-selected word's DOM Range stringifies identically
  // but silently fails to bold every single time regardless of word
  // content or length. Third-party (BlockNote/ProseMirror) behavior, not
  // this app's own code.
  await page.getByText(word, { exact: true }).click({ clickCount: 3 });
  await page.keyboard.press("ControlOrMeta+b");
  await expect(page.locator(".bn-block-content strong", { hasText: word })).toBeVisible();
  await page.getByRole("button", { name: "Done" }).click();

  await expect(page.locator(".wiki-article strong", { hasText: word })).toBeVisible();
});
