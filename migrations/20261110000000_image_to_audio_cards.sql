-- A third media-driven type alongside image_to_target/sound_to_written:
-- image is the prompt, the spoken word is the answer (pronunciation
-- recall, not written recall) — uses the same image_url/audio_url/
-- target_text columns every other type already has, just reviewed with a
-- different prompt/reveal split (see app.rs's ReviewPage). No schema
-- change needed beyond widening the CHECK constraint, which Postgres
-- requires dropping and re-adding rather than altering in place.
ALTER TABLE cards DROP CONSTRAINT cards_card_type_check;
ALTER TABLE cards ADD CONSTRAINT cards_card_type_check
    CHECK (card_type IN ('image_to_target', 'sound_to_written', 'image_to_audio', 'sentence_translation'));
