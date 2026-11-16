-- A video-prompt type: video (an uploaded file or a YouTube link) is the
-- prompt, the written word is the answer. Video and YouTube fields are
-- offered for this type only — see app.rs's `media_fields_for`. Same
-- drop-and-re-add of the CHECK constraint as 20261110000000.
ALTER TABLE cards DROP CONSTRAINT cards_card_type_check;
ALTER TABLE cards ADD CONSTRAINT cards_card_type_check
    CHECK (card_type IN ('image_to_target', 'sound_to_written', 'image_to_audio', 'video_to_written', 'sentence_translation'));
