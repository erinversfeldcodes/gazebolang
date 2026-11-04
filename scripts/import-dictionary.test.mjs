// Unit tests for the pure logic in import-dictionary.mjs — the parts that
// need neither Ollama nor a live app server, so they run fast as a plain
// CI step. The parts that do need a live server (login, createCards,
// saveWordDefinition) are covered instead by an integration test that
// runs the actual `create` stage against a real running instance of the
// app — see e2e/tests/import-dictionary.spec.ts.
import assert from "node:assert/strict";
import { test } from "node:test";
import { cardRejectionReason, extractJsonArray, extractJsonObject, runDirName } from "./import-dictionary.mjs";

test("extractJsonArray parses a clean array", () => {
  assert.deepEqual(extractJsonArray('[{"a":1}]', "ctx"), [{ a: 1 }]);
});

test("extractJsonArray tolerates a markdown fence and surrounding prose", () => {
  assert.deepEqual(
    extractJsonArray('Sure, here it is:\n```json\n[{"a":1},{"b":2}]\n```\nHope that helps!', "ctx"),
    [{ a: 1 }, { b: 2 }],
  );
});

test("extractJsonArray rejects a lone object, not an array", () => {
  assert.throws(() => extractJsonArray('{"a":1}', "ctx"), /couldn't find a JSON array/);
});

test("extractJsonArray rejects unparseable text", () => {
  assert.throws(() => extractJsonArray("not json at all", "ctx"), /couldn't find a JSON array/);
});

test("extractJsonObject parses a clean object", () => {
  assert.deepEqual(extractJsonObject('{"a":1}', "ctx"), { a: 1 });
});

test("extractJsonObject rejects an array, not an object", () => {
  assert.throws(() => extractJsonObject("[1,2]", "ctx"), /couldn't find a JSON object/);
});

test("cardRejectionReason rejects an empty field", () => {
  const seen = new Set();
  assert.equal(
    cardRejectionReason({ english: "", target_text: "x" }, seen),
    "empty or self-referential",
  );
  assert.equal(
    cardRejectionReason({ english: "x", target_text: "" }, seen),
    "empty or self-referential",
  );
});

test("cardRejectionReason rejects english === target_text, case-insensitively", () => {
  const seen = new Set();
  assert.equal(
    cardRejectionReason({ english: "Abandon", target_text: "abandon" }, seen),
    "empty or self-referential",
  );
});

test("cardRejectionReason rejects a leftover ~ in either field", () => {
  const seen = new Set();
  assert.equal(
    cardRejectionReason({ english: "abhorrent", target_text: "~·rent" }, seen),
    "unreconstructed ~ left in output",
  );
  assert.equal(
    cardRejectionReason({ english: "~'d", target_text: "real translation" }, seen),
    "unreconstructed ~ left in output",
  );
});

test("cardRejectionReason accepts a good card and records it", () => {
  const seen = new Set();
  assert.equal(cardRejectionReason({ english: "abandon", target_text: "yeka" }, seen), null);
  assert.equal(seen.size, 1);
});

test("cardRejectionReason rejects an exact repeat of an already-accepted card", () => {
  const seen = new Set();
  assert.equal(cardRejectionReason({ english: "ablaze", target_text: "-vutha" }, seen), null);
  assert.equal(
    cardRejectionReason({ english: "ablaze", target_text: "-vutha" }, seen),
    "duplicate of an already-created card",
  );
});

test("cardRejectionReason treats the duplicate check case-insensitively", () => {
  const seen = new Set();
  assert.equal(cardRejectionReason({ english: "Ablaze", target_text: "-vutha" }, seen), null);
  assert.equal(
    cardRejectionReason({ english: "ablaze", target_text: "-VUTHA" }, seen),
    "duplicate of an already-created card",
  );
});

test("runDirName sanitizes the image basename and appends a timestamp", () => {
  const name = runDirName("/some/path/521e49a4-image.JPG");
  assert.match(name, /^521e49a4-image-\d{4}-\d{2}-\d{2}T/);
});

test("runDirName strips characters a directory name can't safely contain", () => {
  const name = runDirName("my photo (1)!.png");
  assert.doesNotMatch(name, /[()! ]/);
});
