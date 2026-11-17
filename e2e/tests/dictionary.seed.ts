import { test as setup, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import path from "node:path";
import { Client } from "pg";
import { RESEND_ALLOWED_EMAIL, TEST_PASSWORD, makePng } from "./helpers";
import { SEED_DECK, SEED_TAG } from "./seed-data";

// Seeds the database with data shaped like production's, before any spec
// runs. Production's cards all came from the importer (a dictionary entry
// per word, an image card plus a placeholder audio card sharing its
// spelling, tags, decks, related words), whereas specs that create their
// own cards through the UI only ever produce bare cards with no dictionary
// entry — so a whole class of bugs that need real data (a page that failed
// to hydrate on every production card, while passing in CI) could not show
// up. Going through the importer's own `create` stage, not SQL inserts for
// the cards themselves, keeps the seeded rows exactly as the real tool
// writes them; only what that tool doesn't write (tags, a deck, related
// words, an uploaded image) is added afterwards.
const IMPORTER_BIN = path.join(__dirname, "..", "..", "target", "debug", "importer");
const FIXTURE = path.join(__dirname, "..", "fixtures", "dictionary-seed.json");

setup("seed production-shaped dictionary data", async ({ request }) => {
  setup.setTimeout(120_000);
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    const already = await client.query(
      "SELECT 1 FROM card_tags WHERE tag = $1 LIMIT 1",
      [SEED_TAG],
    );
    if (already.rowCount && already.rowCount > 0) return; // reused database

    execFileSync(IMPORTER_BIN, ["create", FIXTURE], {
      env: {
        ...process.env,
        AUTH_EMAIL: RESEND_ALLOWED_EMAIL,
        AUTH_PASSWORD: TEST_PASSWORD,
        BASE_URL: process.env.BASE_URL ?? "http://127.0.0.1:3000",
      },
      stdio: "pipe",
    });

    const fixtureTargets: string[] = (
      JSON.parse(readFileSync(FIXTURE, "utf8")) as Array<{ target_text: string }>
    ).map((c) => c.target_text);

    // Tags and a deck, the way a person organising the imported cards
    // would have: every seeded card tagged by kind, all in one deck.
    const userId = (
      await client.query("SELECT id FROM users WHERE email = $1", [RESEND_ALLOWED_EMAIL])
    ).rows[0].id as string;
    const deckId = (
      await client.query(
        "INSERT INTO decks (name, created_by) VALUES ($1, $2) RETURNING id",
        [SEED_DECK, userId],
      )
    ).rows[0].id as string;
    await client.query(
      `INSERT INTO card_tags (card_id, tag)
         SELECT id, $2::text FROM cards WHERE target_text = ANY($1)
       UNION ALL
         SELECT c.id, COALESCE(NULLIF(w.part_of_speech, ''), 'untagged') FROM cards c
           JOIN words w ON w.target_text = c.target_text WHERE c.target_text = ANY($1)
       ON CONFLICT DO NOTHING`,
      [fixtureTargets, SEED_TAG],
    );
    await client.query(
      `INSERT INTO deck_cards (deck_id, card_id)
         SELECT $2::uuid, id FROM cards WHERE target_text = ANY($1) ON CONFLICT DO NOTHING`,
      [fixtureTargets, deckId],
    );

    // Related words: chain the first few headwords together, as the real
    // dictionary does for synonyms.
    for (let i = 0; i + 1 < 6; i++) {
      await client.query(
        `INSERT INTO word_links (word_id, linked_word_id)
           SELECT a.id, b.id FROM words a, words b
            WHERE a.target_text = $1 AND b.target_text = $2 ON CONFLICT DO NOTHING`,
        [fixtureTargets[i], fixtureTargets[i + 1]],
      );
    }

    // A few cards with a real uploaded image (stored, with a preview) so
    // image pages and the grid are exercised with hosted images too.
    for (const target of fixtureTargets.slice(0, 4)) {
      const card = (
        await client.query(
          "SELECT id FROM cards WHERE target_text = $1 AND card_type = 'image_to_target'",
          [target],
        )
      ).rows[0];
      const res = await request.post(`/cards/${card.id}`, {
        multipart: {
          target_text: target,
          tags: `${SEED_TAG}, seeded-image`,
          card_type: "image_to_target",
          image: {
            name: "seed.png",
            mimeType: "image/png",
            buffer: makePng(900, 600),
          },
        },
      });
      expect(res.ok()).toBeTruthy();
    }
  } finally {
    await client.end();
  }
});
