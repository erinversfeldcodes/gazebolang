import { Page, Response, expect } from "@playwright/test";
import { Client } from "pg";
import * as crypto from "crypto";
import * as zlib from "zlib";

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

// Waits until the page's wasm has taken over its forms. `page.goto` (see
// fixtures.ts) already does this, but a navigation the app makes itself, such
// as the redirect to /login after a password change, loads a new document that
// goto never sees. Submitting a form on it before this resolves falls through
// to a plain POST: the page reloads, the fields are empty, and the next fill
// types into a form that has lost the email.
export async function waitForHydration(page: Page): Promise<void> {
  await page.locator('body[data-hydrated="true"]').waitFor();
}

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

// Like withServerResponse, but resolves on the response whatever its status,
// and returns it. For a server action whose outcome the test asserts on
// afterwards (a sign-up, a login, a password change): the gate is the
// server's answer arriving, not a clock. Hypothesis, not shown: auth.rs
// allows one Argon2 operation at a time (ARGON2_PERMITS = 1), so a hash may
// queue behind other specs'. This helper only waits for the response; it has
// no timeout of its own, so a request that never answers holds the test to its
// full test timeout. Callers keep an explicit timeout on what they assert after.
export async function withServerCall<T>(
  page: Page,
  fnName: string,
  trigger: () => Promise<T>,
): Promise<Response> {
  const [response] = await Promise.all([
    page.waitForResponse((res) => res.url().includes(`/api/${fnName}`)),
    trigger(),
  ]);
  return response;
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

// Writes `count` plain cards carrying `tag` straight into Postgres — far
// faster than creating dozens through the UI, for specs (pagination) that
// only need many cards to exist, not to exercise card creation itself.
export async function seedTaggedCards(tag: string, count: number): Promise<void> {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    const userId = await ensureBootstrapInviter(client);
    for (let i = 0; i < count; i++) {
      const inserted = await client.query(
        "INSERT INTO cards (card_type, target_text, created_by) " +
          "VALUES ('image_to_target', $1, $2) RETURNING id",
        [`${tag}-${String(i).padStart(2, "0")}`, userId],
      );
      await client.query("INSERT INTO card_tags (card_id, tag) VALUES ($1, $2)", [
        inserted.rows[0].id,
        tag,
      ]);
    }
  } finally {
    await client.end();
  }
}

// Removes everything seedTaggedCards wrote for `tag`.
export async function deleteTaggedCards(tag: string): Promise<void> {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    await client.query(
      "DELETE FROM cards WHERE id IN (SELECT card_id FROM card_tags WHERE tag = $1)",
      [tag],
    );
  } finally {
    await client.end();
  }
}

// A valid RGB PNG of random noise — incompressible, so it is genuinely
// large (a 1200x800 one is ~2.9 MB) without needing a fixture file in the
// repo. `noise: false` gives a flat colour instead: tiny on disk.
export function makePng(width: number, height: number, noise = true): Buffer {
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc32 = (buf: Buffer): number => {
    let c = 0xffffffff;
    for (const byte of buf) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type: string, data: Buffer): Buffer => {
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const out = Buffer.alloc(8 + data.length + 4);
    out.writeUInt32BE(data.length, 0);
    body.copy(out, 4);
    out.writeUInt32BE(crc32(body), 8 + data.length);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8; // bit depth
  header[9] = 2; // colour type: RGB
  const stride = width * 3 + 1; // each row starts with a filter byte (0)
  const raw = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    if (noise) {
      crypto.randomFillSync(raw, y * stride + 1, width * 3);
    } else {
      raw.fill(0x7f, y * stride + 1, (y + 1) * stride);
    }
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", zlib.deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// Reads an <img>'s natural (intrinsic) width once it has loaded.
export async function naturalWidth(img: import("@playwright/test").Locator): Promise<number> {
  await expect(img).toBeVisible();
  await expect
    .poll(() => img.evaluate((el: HTMLImageElement) => el.complete && el.naturalWidth))
    .toBeGreaterThan(0);
  return img.evaluate((el: HTMLImageElement) => el.naturalWidth);
}

// Deletes the thumbnail row recorded for `imageUrl`, putting the image back
// in the state of one uploaded before thumbnails existed.
export async function forgetThumbnail(imageUrl: string): Promise<void> {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    await client.query("DELETE FROM image_thumbnails WHERE image_url = $1", [imageUrl]);
  } finally {
    await client.end();
  }
}

// The thumbnail URL recorded for `imageUrl`, or null when there's no row.
export async function recordedThumbnail(imageUrl: string): Promise<string | null> {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    const res = await client.query("SELECT thumb_url FROM image_thumbnails WHERE image_url = $1", [
      imageUrl,
    ]);
    return res.rows.length > 0 ? (res.rows[0].thumb_url as string) : null;
  } finally {
    await client.end();
  }
}
