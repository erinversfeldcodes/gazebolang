-- A small preview for each stored card image, so the card grid and review
-- show a ~20-40 KB thumbnail instead of the full-size (often 200-800 KB)
-- original; the full image is only fetched on a card's own page.
--
-- A separate table keyed by the full image's URL, rather than a column on
-- `cards`, so every existing path that writes `cards.image_url` (upload,
-- imported search result, bulk add, the importer) keeps working untouched
-- and a thumbnail is shared by every card using the same image. A card with
-- no row here simply falls back to its full image. `thumb_url` equal to
-- `image_url` means "already small enough, no separate thumbnail".
CREATE TABLE image_thumbnails (
    image_url TEXT PRIMARY KEY,
    thumb_url TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
