import { test, expect } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { Client } from "pg";
import { RESEND_ALLOWED_EMAIL, TEST_PASSWORD, uniqueWord } from "./helpers";

// Exercises scripts/import-dictionary.mjs's `create` stage end to end
// against this job's real running app + Postgres — the one part of that
// script actually feasible to test in CI. `segment`/`decompose` need
// Ollama plus real local vision/reasoning models (6-20GB each), which CI
// has neither the GPU/RAM budget nor the minutes for; the pure logic
// those two stages share with `create` (JSON extraction, the mechanical
// card filters) is unit-tested directly instead, with no server and no
// Ollama, in scripts/import-dictionary.test.mjs (run via `node --test`).
//
// Deliberately runs the script as a real child process via plain `node`,
// not by importing its functions — that's the same way a downloaded copy
// of the script is actually invoked, so this also catches anything an
// import-level unit test couldn't: wrong CLI arg handling, the real HTTP
// request/cookie shape, login actually succeeding against a live server.
//
// No `page` fixture is used (and so no browser is launched for this
// test) — this is testing an HTTP+CLI tool, not a page.
test("create stage logs in, creates the card, and saves its dictionary details", async () => {
  const targetText = uniqueWord("import-dictionary-it");
  const englishWord = `imported word ${Date.now()}`;

  const dir = mkdtempSync(path.join(tmpdir(), "import-dictionary-it-"));
  const cardsFile = path.join(dir, "cards.json");
  writeFileSync(
    cardsFile,
    JSON.stringify([
      {
        english: englishWord,
        target_text: targetText,
        part_of_speech: "n",
        notes: "created by an e2e integration test",
        example_target: "",
        example_gloss: "",
        image_query: englishWord,
      },
    ]),
  );

  execFileSync(
    "node",
    [path.join(__dirname, "..", "..", "scripts", "import-dictionary.mjs"), "create", cardsFile],
    {
      env: {
        ...process.env,
        AUTH_EMAIL: RESEND_ALLOWED_EMAIL,
        AUTH_PASSWORD: TEST_PASSWORD,
        BASE_URL: process.env.BASE_URL ?? "http://127.0.0.1:3000",
      },
      stdio: "pipe",
    },
  );

  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    const cardRows = await client.query("SELECT target_text FROM cards WHERE target_text = $1", [
      targetText,
    ]);
    expect(cardRows.rowCount).toBe(1);

    const wordRows = await client.query(
      "SELECT english_translation, part_of_speech FROM words WHERE target_text = $1",
      [targetText],
    );
    expect(wordRows.rowCount).toBe(1);
    expect(wordRows.rows[0].english_translation).toBe(englishWord);
    expect(wordRows.rows[0].part_of_speech).toBe("n");
  } finally {
    // Cleanup isn't load-bearing against CI's own ephemeral per-run
    // Postgres, but keeps this safe to run against a longer-lived
    // instance too.
    await client.query("DELETE FROM words WHERE target_text = $1", [targetText]);
    await client.query("DELETE FROM cards WHERE target_text = $1", [targetText]);
    await client.end();
  }
});

test("create stage fails clearly when the login credentials are wrong", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "import-dictionary-it-"));
  const cardsFile = path.join(dir, "cards.json");
  writeFileSync(cardsFile, JSON.stringify([{ english: "x", target_text: "y" }]));

  expect(() =>
    execFileSync(
      "node",
      [path.join(__dirname, "..", "..", "scripts", "import-dictionary.mjs"), "create", cardsFile],
      {
        env: {
          ...process.env,
          AUTH_EMAIL: RESEND_ALLOWED_EMAIL,
          AUTH_PASSWORD: "definitely-the-wrong-password",
          BASE_URL: process.env.BASE_URL ?? "http://127.0.0.1:3000",
        },
        stdio: "pipe",
      },
    ),
  ).toThrow(/login failed/);
});
