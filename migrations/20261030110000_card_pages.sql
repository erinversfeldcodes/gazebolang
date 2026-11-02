-- Links a card to the grammar/theory wiki page(s) that explain it — e.g. a
-- card drilling an -ar verb's present tense links to the "Present tense -ar
-- verbs" page. Mirrors deck_cards exactly: a plain many-to-many join table
-- with no extra columns, cascading from either side.
CREATE TABLE card_pages (
    card_id UUID NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    page_id UUID NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    PRIMARY KEY (card_id, page_id)
);

CREATE INDEX card_pages_page_id_idx ON card_pages (page_id);
