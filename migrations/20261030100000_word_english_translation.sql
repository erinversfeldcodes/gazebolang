-- A dictionary entry had part_of_speech/notes/examples but no field for
-- the headword's own English translation — only an example sentence's
-- gloss_sentence carried any English text, and only if an example was
-- filled in. Nullable, same convention as part_of_speech/notes: a stub
-- entry (created only to satisfy a word_links row, via
-- resolve_or_create_word_id_in_tx) has no translation yet either.
ALTER TABLE words ADD COLUMN english_translation TEXT;
