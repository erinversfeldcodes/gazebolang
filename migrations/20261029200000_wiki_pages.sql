-- The grammar/theory content system: a Confluence/Wikipedia-style page tree,
-- editable at runtime (not a fixed set of routes). parent_id is nullable so
-- multiple root-level pages can coexist, each ordered among its siblings
-- (including other roots) via position.
--
-- current_revision_id is nullable only to break the circular reference with
-- page_revisions below — app code always inserts a page, then its first
-- revision, then points the page at it, all in one transaction.
CREATE TABLE pages (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    parent_id UUID REFERENCES pages(id),
    slug TEXT NOT NULL,
    title TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    current_revision_id UUID,
    -- Denormalized (title + content_markdown of the current revision), kept
    -- in sync by app code in the same transaction that repoints
    -- current_revision_id. Not a GENERATED column: Postgres generated
    -- columns can't reference another table, and a trigger is more
    -- machinery than this app uses elsewhere (see the cards table's
    -- 4-user-cap comment) for what's already just one more field in an
    -- update that's happening anyway.
    search_text TEXT NOT NULL DEFAULT '',
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Append-only, like review_log — restoring an old version creates a new
-- revision rather than rewriting history. content_json is the canonical
-- BlockNote document; content_markdown is a derived export (markdown
-- round-tripping is lossy, so it's never the source of truth) kept for
-- revision diffing, search indexing, and server-rendering read-only views
-- without shipping the editor's JS to people who are just reading.
CREATE TABLE page_revisions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    page_id UUID NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    author_id UUID NOT NULL REFERENCES users(id),
    title TEXT NOT NULL,
    content_json JSONB NOT NULL,
    content_markdown TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE pages
    ADD CONSTRAINT pages_current_revision_id_fkey
    FOREIGN KEY (current_revision_id) REFERENCES page_revisions(id);

-- Mirrors card_tags — flat, cross-cutting categories alongside the tree.
CREATE TABLE page_tags (
    page_id UUID NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    PRIMARY KEY (page_id, tag)
);

CREATE INDEX page_tags_tag_idx ON page_tags (tag);
CREATE INDEX pages_parent_id_idx ON pages (parent_id, position);
CREATE INDEX page_revisions_page_id_created_at_idx ON page_revisions (page_id, created_at DESC);

-- Full-text search over pages.search_text. Postgres FTS rather than a
-- separate engine like tantivy: this app already lives entirely in
-- Postgres, and a tantivy index would need its own persistent volume on
-- Fly (fly.toml runs auto_stop_machines/min_machines_running = 0 — the
-- container filesystem isn't reliably persistent) or a rebuild-on-boot
-- step, real new infrastructure at a scale (a handful of users, a hobby
-- project's worth of articles) nowhere near where a dedicated search
-- engine's performance edge would matter.
CREATE INDEX pages_search_idx ON pages USING GIN (to_tsvector('english', search_text));
