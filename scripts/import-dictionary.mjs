#!/usr/bin/env node
// Prototype: read photographed dictionary page(s), using *local* vision/
// text models served by Ollama (no third-party API — deliberate, since the
// point of this prototype is to find out whether local reading is good
// enough before this becomes a real feature the deployed app depends on),
// and create real, fully-detailed flashcards from them — no new app code,
// no job queue yet. Review and fix the results afterward in the running
// app's normal UI.
//
// Three stages, run as three separate OS processes (`segment`, `decompose`,
// `create` — `all`, the default, just runs all three as child processes in
// order), not three functions in one process:
//   1. segment    Vision pass. Read a photo into one {headword, raw_entry}
//                  block per bold headword, verbatim, no restructuring.
//   2. decompose   Per-entry, per-item text pass (no image). Two sub-calls
//                  per entry, not one: first *enumerate* the entry's
//                  distinct learnable units (senses, sub-senses, worked
//                  examples, derived words) as short labels, then *fill in*
//                  each one's full card fields with its own small call.
//                  Splitting it this way — rather than asking for a whole
//                  JSON array of multi-field cards in one generation — is
//                  deliberate: that one-big-array shape is exactly what
//                  caused inconsistent output under full-batch load
//                  (hallucinated derived words, phrase-shattering,
//                  self-referential cards) when this was first tried, and
//                  it's also the shape that llama.cpp's own grammar sampler
//                  has a documented, still-open performance cliff on
//                  (ggml-org/llama.cpp#4218) when `format` below is used.
//   3. create      Image lookup (Pixabay) + POST to the app. No Ollama
//                  call at all, so it never competes with either model for
//                  memory.
// Running each stage as its own process (rather than switching models
// within one long-lived process) is deliberate too: this host only has
// 24GB RAM, and letting one model's Ollama runner stay resident while a
// second, differently-sized model loads for the next stage pushed the
// whole system into a low-memory kill, twice, even though neither model
// alone was the problem. Each stage also calls Ollama's unload endpoint
// before exiting as a second line of defense (`keep_alive: 0` — see
// unloadModel below).
//
// Every model call also passes Ollama's `format` field — a real JSON
// Schema, not just the string "json" — which Ollama compiles to a GBNF
// grammar and uses to constrain token sampling (confirmed: ollama/ollama PR
// #7900), not just a prompting nudge. This guarantees valid JSON *shape*;
// it does nothing for hallucinated *content*, which is why the mechanical
// filter below (reject target_text === english) and the enumerate/fill
// split above still both matter. If a schema-constrained call still fails
// to parse — observed as a real, documented failure mode for some models
// (gpt-oss's "Harmony" response format fighting structured output, e.g.
// ollama/ollama#11691) — chatJson() retries once with the same prompt and
// no schema before giving up on that one item/entry.
//
// Usage:
//   ollama pull qwen2.5vl:7b    # once — segment (vision)
//   ollama pull gpt-oss:20b     # once — decompose (text reasoning)
//   PIXABAY_API_KEY=... node scripts/import-dictionary.mjs photo1.jpg photo2.jpg
//
// Or run/resume one stage at a time (useful if a long run fails partway —
// `all` prints the run directory it's using, which holds entries.json and
// cards.json so you can inspect or hand-edit them between stages):
//   node scripts/import-dictionary.mjs segment    <entries.json> <photo.jpg...>
//   node scripts/import-dictionary.mjs decompose  <entries.json> <cards.json>
//   node scripts/import-dictionary.mjs create     <cards.json>
//
// Env vars:
//   OLLAMA_HOST             default http://127.0.0.1:11434
//   OLLAMA_SEGMENT_MODEL    default qwen2.5vl:7b — pass 1 (page -> raw
//                           per-headword blocks). A reading task; 7b read a
//                           dense real page completely and accurately.
//   OLLAMA_DECOMPOSE_MODEL  default gpt-oss:20b — pass 2 (one entry's raw
//                           text -> several sense/derived-word cards). This
//                           is a reasoning task, not a reading task, so it
//                           uses a dedicated text model rather than asking
//                           a vision model to do pure text reasoning — a
//                           vision-capable model (qwen2.5vl:7b) was
//                           confirmed inconsistent at it under full-batch
//                           load even though isolated tests looked fine.
//                           Also worth trying here: NuExtract3, a small
//                           (~4B, well under 3GB resident) model fine-tuned
//                           specifically for text-to-JSON extraction —
//                           `ollama run hf.co/numind/NuExtract3-GGUF:Q4_K_M`
//                           — vendor-benchmarked only, not independently
//                           verified, but a cheap swap-in to try since
//                           nothing else about the pipeline needs to change.
//   PIXABAY_API_KEY         same key the app's own PIXABAY_API_KEY uses — omit
//                           and cards are still created, just with no image
//   BASE_URL                default http://127.0.0.1:3000 — your local
//                           `cargo leptos watch` server, or a real deployed
//                           instance of the app once you're pointing this
//                           at one.
//   AUTH_EMAIL / AUTH_PASSWORD  required (only `create` actually uses
//                           them). Logs in for real, the same way the
//                           app's own login form does — no browser, no
//                           e2e test infrastructure, so this also works
//                           against a real deployed app, not just a local
//                           dev server with an existing Playwright-created
//                           session file. Logging in fresh every run
//                           (rather than caching the resulting cookie) is
//                           deliberate: a stale cached session cookie is
//                           exactly what broke a `create` run partway
//                           through once already, hours into a run — a
//                           login call is cheap, losing an hour of
//                           decompose output to an expired cached cookie
//                           is not.
//
// This is slow — a real page runs one vision call plus two text calls per
// learnable unit (dozens of sequential Ollama calls), easily 10+ minutes
// for a dense page. That's fine for "let it run and review after"; it's
// not meant to be interactive.

import { spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const here = path.dirname(scriptPath);

const BASE_URL = process.env.BASE_URL ?? "http://127.0.0.1:3000";
const OLLAMA_HOST = process.env.OLLAMA_HOST ?? "http://127.0.0.1:11434";
const OLLAMA_SEGMENT_MODEL = process.env.OLLAMA_SEGMENT_MODEL ?? "qwen2.5vl:7b";
const OLLAMA_DECOMPOSE_MODEL = process.env.OLLAMA_DECOMPOSE_MODEL ?? "gpt-oss:20b";
const AUTH_EMAIL = process.env.AUTH_EMAIL;
const AUTH_PASSWORD = process.env.AUTH_PASSWORD;
const PIXABAY_API_KEY = process.env.PIXABAY_API_KEY;
// Ollama defaults to a 4096-token context window regardless of what the
// model itself supports — the image alone can exceed that on a real photo
// (confirmed live: "request (4115 tokens) exceeds the available context
// size (4096 tokens)" on a single dense dictionary page). Must be
// requested explicitly per call via `options.num_ctx`.
const OLLAMA_NUM_CTX = Number(process.env.OLLAMA_NUM_CTX ?? 16384);

const SUPPORTED_EXTENSIONS = [".jpg", ".jpeg", ".png", ".webp"];

// Leptos #[server] fns' real endpoint paths, baked into the compiled WASM
// as stable constants (found via `strings` on /pkg/web.wasm — there's no
// documented stable path for these, so each is re-derivable the same way
// if the function's containing source file is ever renamed/moved, which
// is what the numeric suffix is actually keyed on, not the function name
// itself: `strings target/site/pkg/web.wasm | grep -o '/api/[a-zA-Z_]*[0-9]\{10,\}'`
// dumps the full current map).
const SAVE_WORD_DEFINITION_PATH = "/api/save_word_definition6677106481201695198";
const LOG_IN_PATH = "/api/log_in6363758671868516822";

const SEGMENT_SCHEMA = {
  type: "array",
  items: {
    type: "object",
    properties: {
      headword: { type: "string" },
      raw_entry: { type: "string" },
    },
    required: ["headword", "raw_entry"],
  },
};

const ENUMERATE_SCHEMA = {
  type: "array",
  items: {
    type: "object",
    properties: {
      label: { type: "string" },
      source_excerpt: { type: "string" },
    },
    required: ["label", "source_excerpt"],
  },
};

const FILL_SCHEMA = {
  type: "object",
  properties: {
    english: { type: "string" },
    target_text: { type: "string" },
    part_of_speech: { type: "string" },
    notes: { type: "string" },
    example_target: { type: "string" },
    example_gloss: { type: "string" },
    image_query: { type: "string" },
  },
  required: ["english", "target_text"],
};

// Same request shape as save_word_definition — a bare #[server] fn's
// default encoding is a url-encoded POST, confirmed for log_in
// specifically too, not just assumed by analogy. A successful call sets
// a `session=<token>` cookie (same cookie name/mechanism this script's
// other calls already send back); a failure (wrong password, no such
// user — deliberately indistinguishable, see auth.rs) comes back as a
// non-2xx status with "invalid email or password" in the body.
async function login() {
  if (!AUTH_EMAIL || !AUTH_PASSWORD) {
    throw new Error("AUTH_EMAIL and AUTH_PASSWORD env vars are required to create cards");
  }
  const res = await fetch(`${BASE_URL}${LOG_IN_PATH}`, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ email: AUTH_EMAIL, password: AUTH_PASSWORD }).toString(),
  });
  const setCookies = res.headers.getSetCookie?.() ?? [];
  const sessionCookie = setCookies
    .map((c) => c.split(";")[0])
    .find((c) => c.startsWith("session="));
  if (!res.ok || !sessionCookie) {
    const text = await res.text().catch(() => "");
    throw new Error(`login failed (${res.status}): ${text || "no session cookie in response"}`);
  }
  return sessionCookie.slice("session=".length);
}

// Deliberately node:http, not global fetch: fetch's underlying client
// (undici) hard-caps headers+body wait at 5 minutes with no way to raise it
// from script code without an npm dependency, and a big model's
// image-decode-plus-generate time on a real dense photo routinely exceeds
// that (confirmed live: three straight attempts each killed at exactly
// "5m0s" per the Ollama server log, every one otherwise still progressing
// normally server-side). node:http's client has no such timeout by default.
function postJson(urlStr, bodyObj) {
  return new Promise((resolve, reject) => {
    const body = JSON.stringify(bodyObj);
    const req = http.request(
      new URL(urlStr),
      {
        method: "POST",
        headers: { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(body) },
      },
      (res) => {
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () =>
          resolve({ status: res.statusCode, text: Buffer.concat(chunks).toString("utf8") }),
        );
      },
    );
    req.on("error", reject);
    req.write(body);
    req.end();
  });
}

// A full page is dozens of sequential calls over 15-30+ minutes — a single
// transient connection blip (confirmed live: a bare "fetch failed" with no
// HTTP status, while the Ollama daemon and model runner were both still
// healthy) shouldn't lose the whole run. Three attempts with a short
// backoff; only gives up after all three fail.
async function ollamaChat(model, messages, schema, attempt = 1) {
  let res;
  try {
    res = await postJson(`${OLLAMA_HOST}/api/chat`, {
      model,
      stream: false,
      // temperature: 0 for determinism — small models are measurably more
      // prone to structural/content errors at higher temperatures on
      // exactly this kind of multi-field extraction task.
      options: { num_ctx: OLLAMA_NUM_CTX, temperature: 0 },
      messages,
      ...(schema ? { format: schema } : {}),
    });
  } catch (e) {
    if (attempt >= 3) throw new Error(`ollama request failed after 3 attempts: ${e.message}`);
    console.warn(`  ollama request failed (attempt ${attempt}/3: ${e.message}), retrying...`);
    await new Promise((resolve) => setTimeout(resolve, 2000 * attempt));
    return ollamaChat(model, messages, schema, attempt + 1);
  }
  if (res.status !== 200) {
    throw new Error(
      `ollama request failed (${res.status}) — is \`ollama serve\` running and has ` +
        `\`ollama pull ${model}\` been run? ${res.text}`,
    );
  }
  const { message } = JSON.parse(res.text);
  return (message?.content ?? "").trim();
}

// Unloads a model immediately rather than waiting out Ollama's default
// keep-alive window — best-effort hygiene between stages so two
// differently-sized models are never resident at once on a host with only
// 24GB RAM. Not worth failing a run over if it doesn't work.
async function unloadModel(model) {
  try {
    await postJson(`${OLLAMA_HOST}/api/generate`, { model, prompt: "", keep_alive: 0 });
  } catch {
    // best-effort
  }
}

// Smaller local models sometimes still wrap output in a markdown fence or
// add a sentence before/after it even under schema-constrained decoding —
// pull out the first matching span rather than requiring the whole
// response to be clean JSON.
function extractJsonArray(text, context) {
  const match = text.match(/\[[\s\S]*\]/);
  if (!match) {
    throw new Error(`couldn't find a JSON array in ${context}'s response:\n${text}`);
  }
  try {
    const parsed = JSON.parse(match[0]);
    if (!Array.isArray(parsed)) throw new Error("not an array");
    return parsed;
  } catch (e) {
    throw new Error(`couldn't parse ${context}'s response: ${e.message}\n---\n${text}`);
  }
}

function extractJsonObject(text, context) {
  const match = text.match(/\{[\s\S]*\}/);
  if (!match) {
    throw new Error(`couldn't find a JSON object in ${context}'s response:\n${text}`);
  }
  try {
    const parsed = JSON.parse(match[0]);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      throw new Error("not an object");
    }
    return parsed;
  } catch (e) {
    throw new Error(`couldn't parse ${context}'s response: ${e.message}\n---\n${text}`);
  }
}

// Schema-constrained call with one unconstrained fallback. `format`
// guarantees shape when it works, but some models' response formats fight
// it outright (e.g. gpt-oss's "Harmony" channels — ollama/ollama#11691),
// which shows up here as a response that fails to parse at all. Rather
// than losing that one item/entry, retry once with the same prompt, no
// schema, and an explicit plain-JSON instruction appended.
async function chatJson(model, message, schema, context, { array = false } = {}) {
  try {
    const text = await ollamaChat(model, [message], schema);
    return array ? extractJsonArray(text, context) : extractJsonObject(text, context);
  } catch (e) {
    console.warn(`  ${context}: schema-constrained call failed (${e.message}), retrying unconstrained...`);
    const fallbackMessage = {
      ...message,
      content: `${message.content}\n\nRespond with ONLY ${array ? "a JSON array" : "a single JSON object"}, no markdown fence, no commentary.`,
    };
    const text = await ollamaChat(model, [fallbackMessage], null);
    return array ? extractJsonArray(text, context) : extractJsonObject(text, context);
  }
}

// Pass 1 — read one photo into {headword, raw_entry} blocks, verbatim, no
// restructuring. Validated live: this plain segmentation prompt correctly
// found every one of 29 headwords on a dense real page, including ones a
// "pair each headword with just its translation" one-shot prompt had
// dropped outright.
async function segmentPage(imagePath) {
  const ext = path.extname(imagePath).toLowerCase();
  if (!SUPPORTED_EXTENSIONS.includes(ext)) {
    throw new Error(`unrecognized image type for ${imagePath} (expected .jpg/.jpeg/.png/.webp)`);
  }
  const data = (await fs.readFile(imagePath)).toString("base64");

  const entries = await chatJson(
    OLLAMA_SEGMENT_MODEL,
    {
      role: "user",
      content:
        "This is a photo of a page from a dictionary. Each entry starts with a bold " +
        "headword. Transcribe the page into one object per bold headword: " +
        '{"headword": "<the headword exactly as printed>", "raw_entry": "<the ' +
        "complete text of that entry, from the headword up to (not including) the " +
        'next bold headword, transcribed exactly as printed, verbatim — do not ' +
        "summarize, reorder, or drop anything>\"}. Transcribe every headword on the " +
        "page, even ones you're only partly confident about — getting the " +
        "segmentation boundaries right matters more than getting every character " +
        "perfect.",
      images: [data],
    },
    SEGMENT_SCHEMA,
    imagePath,
    { array: true },
  );
  return entries.filter((e) => e && e.headword && e.raw_entry);
}

// Pass 2a — enumerate one already-correctly-read entry's distinct
// learnable units (not their fields yet, just what they are): the
// headword's own sense(s), each parenthetical sub-sense, each worked
// example phrase, and each derived word. Kept separate from filling in
// each one's fields (below) so no single call has to get many multi-field
// records right at once — that one-big-array shape is what produced
// hallucinated derived words and phrase-shattering when tried directly.
async function enumerateItems(headword, rawEntry) {
  const items = await chatJson(
    OLLAMA_DECOMPOSE_MODEL,
    {
      role: "user",
      content:
        "Below is one raw, correctly-transcribed dictionary entry (it may cover " +
        "several related words). List every distinct learnable unit in it as a " +
        "short item — you are NOT filling in translations yet, just identifying " +
        "what the separate flashcards should be. That means:\n" +
        "- The headword's own general sense(s), each as one item.\n" +
        '- Each distinct sense given in parentheses (e.g. "(skilful)", "(of ' +
        'conditions)"), each as one item.\n' +
        '- Each worked example ("eg. ...") that is itself a useful standalone ' +
        "phrase, as one item.\n" +
        "- Each derived word mentioned (spelled out in full, or abbreviated with a " +
        "~ standing in for the headword — reconstruct its full spelling, never " +
        "leave a ~ in your output), each as one item.\n\n" +
        "Do NOT invent an item for a derived word that isn't actually written in " +
        "the entry below.\n\n" +
        'For EACH item, output an object with "label" (a short description of ' +
        'what this card teaches, e.g. "abandon (verb)", "abandoned (adj, given up ' +
        'to bad ways)", "ability (noun)") and "source_excerpt" (the exact ' +
        "substring of the entry below that this item comes from, copied " +
        "verbatim).\n\n" +
        `Entry (headword: "${headword}"):\n${rawEntry}`,
    },
    ENUMERATE_SCHEMA,
    `${headword} (enumerate)`,
    { array: true },
  );
  return items.filter((i) => i && i.label && i.source_excerpt);
}

// Pass 2b — fill in one previously-enumerated item's full card fields. One
// small call per item rather than one big call per entry, for the same
// reason as the enumerate/fill split itself: a single-object generation is
// far more reliable than a multi-object array at this model scale.
async function fillItem(headword, rawEntry, item) {
  return chatJson(
    OLLAMA_DECOMPOSE_MODEL,
    {
      role: "user",
      content:
        "Below is one raw, correctly-transcribed dictionary entry, and ONE " +
        "specific item within it that you previously identified as its own " +
        "flashcard. Fill in that one card's fields — do not produce cards for " +
        "any other part of the entry.\n\n" +
        'Output an object with: "english" (a short, clean English label for ' +
        "exactly what this card teaches — just the word or phrase, no part-of-" +
        'speech abbreviation baked in), "target_text" (the exact target-language ' +
        "text for just this card — never paste in content for a different sense/" +
        'word), "part_of_speech" (if known, else ""), "notes" (brief extra ' +
        'context if useful, else ""), "example_target" and "example_gloss" (if ' +
        'this card itself is built from a worked example, else both ""), and ' +
        '"image_query" (a short, concrete, literally-photographable English ' +
        "phrase illustrating this specific card's meaning — for an abstract word, " +
        'describe a scene that conveys it, e.g. "aback" -> "person looking ' +
        'shocked and surprised", not the word itself).\n\n' +
        "target_text must never be identical to english — if you can't find an " +
        'actual translation, leave target_text as "" rather than echoing English ' +
        "or an unrelated fragment back as both fields.\n\n" +
        `Full entry (headword: "${headword}"):\n${rawEntry}\n\n` +
        `The one item to fill in:\nLabel: ${item.label}\nExcerpt: ${item.source_excerpt}`,
    },
    FILL_SCHEMA,
    `${headword} / ${item.label}`,
    { array: false },
  );
}

// Same Pixabay search the app's own image_search.rs calls, taking the top
// hit's full-resolution URL directly rather than going through the
// interactive thumbnail-then-fetch_by_id picker flow — there's no human
// in the loop here to pick one, and this script's whole premise is that a
// human reviews/corrects the result afterward in the app itself.
async function findImageUrl(query) {
  if (!PIXABAY_API_KEY) return null;
  const url = new URL("https://pixabay.com/api/");
  url.searchParams.set("key", PIXABAY_API_KEY);
  url.searchParams.set("q", query);
  url.searchParams.set("image_type", "photo");
  url.searchParams.set("per_page", "3"); // Pixabay rejects per_page < 3
  const res = await fetch(url);
  if (!res.ok) {
    console.warn(`  pixabay search for "${query}" failed: ${res.status}`);
    return null;
  }
  const { hits } = await res.json();
  return hits?.[0]?.largeImageURL ?? null;
}

// Posts straight to the same /cards/bulk route BulkAddPage's form submits
// to — reuses all of its existing create_card/validation logic, no new
// app code. That handler always redirects (to "/" on success, back to
// "/bulk-add?error=..." on failure, or to "/login" if the cookie's no
// good) rather than returning a 200, so the redirect target is what we
// check, not the status code.
async function createCards(cookie, cards) {
  const form = new FormData();
  cards.forEach((c, i) => {
    form.set(`target_text_${i}`, c.target_text);
    if (c.imageUrl) form.set(`image_url_${i}`, c.imageUrl);
  });

  const res = await fetch(`${BASE_URL}/cards/bulk`, {
    method: "POST",
    headers: { Cookie: `session=${cookie}` },
    body: form,
    redirect: "manual",
  });
  const location = res.headers.get("location") ?? "";
  if (location.startsWith("/login")) {
    throw new Error("session cookie is invalid or expired — re-run the e2e auth setup");
  }
  if (location.includes("error=")) {
    throw new Error(`the app rejected the batch: ${decodeURIComponent(location)}`);
  }
}

// Populates the same "Dictionary details" popup the edit page's
// DictionaryDetailsEditor saves to, keyed by exact target_text (an
// upsert — see dictionary.rs::save_word_definition's own doc comment).
// /cards/bulk gives back no card ids, but none are needed here: this
// writes to the standalone `words` table, looked up by target_text at
// render time, not tied to a specific card row.
async function saveWordDefinition(cookie, card) {
  const body = new URLSearchParams({
    target_text: card.target_text,
    part_of_speech: card.part_of_speech ?? "",
    notes: card.notes ?? "",
    english_translation: card.english ?? "",
    example_target_sentence: card.example_target ?? "",
    example_gloss_sentence: card.example_gloss ?? "",
    linked_words_raw: "",
  });
  const res = await fetch(`${BASE_URL}${SAVE_WORD_DEFINITION_PATH}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/x-www-form-urlencoded",
      Cookie: `session=${cookie}`,
    },
    body: body.toString(),
  });
  if (!res.ok) {
    console.warn(`  dictionary details save failed for "${card.target_text}": ${res.status}`);
  }
}

async function runSegment(entriesOutFile, imagePaths) {
  const entries = [];
  for (const imagePath of imagePaths) {
    console.log(`reading ${imagePath} with ${OLLAMA_SEGMENT_MODEL}...`);
    const found = await segmentPage(imagePath);
    console.log(`  found ${found.length} headword(s)`);
    entries.push(...found);
  }
  await unloadModel(OLLAMA_SEGMENT_MODEL);
  await fs.mkdir(path.dirname(entriesOutFile), { recursive: true });
  await fs.writeFile(entriesOutFile, JSON.stringify(entries, null, 2));
  console.log(`wrote ${entries.length} entries to ${entriesOutFile}`);
}

// Returns a short, human-readable reason to reject a drafted card, or null
// if it should be kept. Not fully pure — a kept card's key is recorded
// into `seenCardKeys` so a later duplicate of it is caught too — but
// synchronous and needs no Ollama/network call, so it's unit-tested on
// its own rather than only indirectly through a full decompose run.
function cardRejectionReason(card, seenCardKeys) {
  const english = (card.english ?? "").trim();
  const targetText = (card.target_text ?? "").trim();
  if (!english || !targetText || english.toLowerCase() === targetText.toLowerCase()) {
    return "empty or self-referential";
  }
  if (english.includes("~") || targetText.includes("~")) {
    return "unreconstructed ~ left in output";
  }
  // Keyed on the pair actually shown to the learner (english + target_text),
  // not the item label — two different enumerate labels (e.g. "ablaze" and
  // "ablaze (excited)") can still land on the identical card when the entry
  // text overlaps, and that's the duplicate worth catching.
  const key = `${english.toLowerCase()}|${targetText.toLowerCase()}`;
  if (seenCardKeys.has(key)) {
    return "duplicate of an already-created card";
  }
  seenCardKeys.add(key);
  return null;
}

async function runDecompose(entriesInFile, cardsOutFile) {
  const entries = JSON.parse(await fs.readFile(entriesInFile, "utf8"));
  const cards = [];
  const seenCardKeys = new Set();
  if (entries.length > 0) {
    console.log(`splitting ${entries.length} entries into cards with ${OLLAMA_DECOMPOSE_MODEL}...`);
    for (const entry of entries) {
      try {
        const items = await enumerateItems(entry.headword, entry.raw_entry);
        console.log(`  ${entry.headword}: ${items.length} item(s)`);
        for (const item of items) {
          try {
            const card = await fillItem(entry.headword, entry.raw_entry, item);
            const reason = cardRejectionReason(card, seenCardKeys);
            if (reason) {
              console.warn(`    "${item.label}": skipped (${reason})`);
              continue;
            }
            cards.push(card);
          } catch (e) {
            // One item failing shouldn't lose the rest of this entry's
            // already-identified items.
            console.warn(`    "${item.label}": skipped (${e.message})`);
          }
        }
      } catch (e) {
        // One entry failing even after retries shouldn't lose everything
        // already extracted from the rest of the page.
        console.warn(`  ${entry.headword}: skipped (${e.message})`);
      }
    }
  }
  await unloadModel(OLLAMA_DECOMPOSE_MODEL);
  await fs.mkdir(path.dirname(cardsOutFile), { recursive: true });
  await fs.writeFile(cardsOutFile, JSON.stringify(cards, null, 2));
  console.log(`wrote ${cards.length} card(s) to ${cardsOutFile}`);
}

async function runCreate(cardsInFile) {
  const cards = JSON.parse(await fs.readFile(cardsInFile, "utf8"));
  if (cards.length === 0) {
    console.log("nothing to create — exiting.");
    return;
  }
  if (!PIXABAY_API_KEY) {
    console.warn("PIXABAY_API_KEY is not set — cards will be created with no image");
  }
  const cookie = await login();

  console.log(`looking up images for ${cards.length} card(s)...`);
  for (const c of cards) {
    c.imageUrl = await findImageUrl(c.image_query || c.english);
    console.log(`  ${c.english} -> ${c.target_text}${c.imageUrl ? "" : " (no image found)"}`);
  }

  console.log(`creating ${cards.length} card(s)...`);
  await createCards(cookie, cards);

  console.log("saving dictionary details...");
  for (const c of cards) {
    await saveWordDefinition(cookie, c);
  }

  console.log(`done — open ${BASE_URL}/ to review the new cards.`);
}

function runDirName(firstImagePath) {
  const base = path
    .basename(firstImagePath)
    .replace(/\.[^.]+$/, "")
    .replace(/[^a-zA-Z0-9_-]/g, "-");
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  return `${base}-${stamp}`;
}

function runChildStage(args) {
  const res = spawnSync(process.execPath, [scriptPath, ...args], { stdio: "inherit" });
  if (res.status !== 0) {
    throw new Error(
      `stage "${args[0]}" failed (exit ${res.status}) — fix the issue, then resume with:\n` +
        `  node ${path.relative(process.cwd(), scriptPath)} ${args.join(" ")}`,
    );
  }
}

async function runAll(imagePaths) {
  if (imagePaths.length === 0) {
    console.error("usage: node scripts/import-dictionary.mjs <photo1.jpg> [photo2.jpg ...]");
    process.exit(1);
  }
  // Fail fast on missing/wrong credentials before spending 10+ minutes on
  // segment+decompose, even though `create` (the only stage that actually
  // needs them) runs last. Logs in again, fresh, when `create` itself
  // runs — see login()'s own comment on why that's not wasteful here.
  await login();

  const runDir = path.join(here, ".import-runs", runDirName(imagePaths[0]));
  const entriesFile = path.join(runDir, "entries.json");
  const cardsFile = path.join(runDir, "cards.json");
  console.log(`run directory: ${runDir}`);

  runChildStage(["segment", entriesFile, ...imagePaths]);
  runChildStage(["decompose", entriesFile, cardsFile]);
  runChildStage(["create", cardsFile]);
}

async function main() {
  const argv = process.argv.slice(2);
  const KNOWN = new Set(["segment", "decompose", "create", "all"]);
  const cmd = KNOWN.has(argv[0]) ? argv[0] : "all";
  const args = KNOWN.has(argv[0]) ? argv.slice(1) : argv;

  switch (cmd) {
    case "segment": {
      const [entriesOutFile, ...imagePaths] = args;
      if (!entriesOutFile || imagePaths.length === 0) {
        console.error("usage: node import-dictionary.mjs segment <entries.json> <photo1.jpg> [photo2.jpg ...]");
        process.exit(1);
      }
      await runSegment(entriesOutFile, imagePaths);
      return;
    }
    case "decompose": {
      const [entriesInFile, cardsOutFile] = args;
      if (!entriesInFile || !cardsOutFile) {
        console.error("usage: node import-dictionary.mjs decompose <entries.json> <cards.json>");
        process.exit(1);
      }
      await runDecompose(entriesInFile, cardsOutFile);
      return;
    }
    case "create": {
      const [cardsInFile] = args;
      if (!cardsInFile) {
        console.error("usage: node import-dictionary.mjs create <cards.json>");
        process.exit(1);
      }
      await runCreate(cardsInFile);
      return;
    }
    case "all":
    default: {
      await runAll(args);
      return;
    }
  }
}

// Guards the CLI entry point so this file can also be `import`ed (e.g. by
// scripts/import-dictionary.test.mjs, via `node --test`) to unit-test the
// functions exported below without spawning the actual pipeline — matches
// `process.argv[1]` to this file's own path, which is false when some
// other file (a test runner) is what Node was actually invoked on. Also
// correctly true for a child-stage invocation: runChildStage() spawns
// `node ${scriptPath} <stage> ...`, so process.argv[1] is this same path.
if (process.argv[1] === scriptPath) {
  main().catch((err) => {
    console.error(err.message);
    process.exit(1);
  });
}

export { extractJsonArray, extractJsonObject, cardRejectionReason, runDirName };
