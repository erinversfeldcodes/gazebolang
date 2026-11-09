use leptos::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(feature = "ssr")]
use serde_json::Value;
#[cfg(feature = "ssr")]
use std::collections::HashMap;
#[cfg(feature = "ssr")]
use std::fmt::Write as _;
use uuid::Uuid;

/// A tree node with no content — what the sidebar renders. `parent_id` is
/// `None` for a root-level page (the tree can have more than one root).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageTreeNode {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub slug: String,
    pub title: String,
    pub position: i32,
}

/// A page's current content — `content_json` is the canonical `BlockNote`
/// document (what the editor loads), `content_markdown` is a derived export
/// (what a read-only, signed-out view renders, and what search/diffing use).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageContent {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub slug: String,
    pub title: String,
    pub content_json: String,
    pub content_markdown: String,
    /// `content_json` rendered to HTML server-side (via
    /// `render_blocks_html`, then `with_table_of_contents`, then
    /// `with_footnotes`) — what the read-only view injects with
    /// `inner_html`, so a plain visitor never has to load the editor's JS
    /// just to read a page. Already includes a "Contents" box if the page
    /// has one (inserted via the editor's Insert menu, or added
    /// automatically for a page with 2+ headings and no explicit
    /// placement) and a generated "Footnotes" list at the end if the page
    /// has any — nothing further to render for either.
    pub content_html: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TocEntry {
    pub level: u8,
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSearchResult {
    pub id: Uuid,
    pub slug: String,
    pub title: String,
    /// Already has the matched terms wrapped in `<b>...</b>` (from
    /// Postgres's `ts_headline`) — render with `inner_html`, not as plain
    /// text, or the highlighting is just literal angle brackets on screen.
    pub snippet: String,
}

/// A card that cites this page — just enough for a backlink list
/// (`EditCardPage` is the only place a card's own detail lives).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedCard {
    pub id: Uuid,
    pub target_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRevisionSummary {
    pub id: Uuid,
    pub author_id: Uuid,
    pub title: String,
    /// RFC3339, formatted server-side — `OffsetDateTime` itself has no
    /// precedent for crossing the wire anywhere else in this codebase, so
    /// this keeps the client-facing type unambiguous rather than leaning
    /// on `time`'s serde impl doing the right thing by default.
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionDiff {
    pub current_title: String,
    /// Line-level diff against the revision immediately before this one
    /// (or against an empty page, for a first revision), rendered to HTML
    /// server-side with changed lines wrapped in `<ins>`/`<del>` — render
    /// with `inner_html`, matching `PageContent::content_html`.
    pub diff_html: String,
}

#[cfg(feature = "ssr")]
async fn require_user() -> Result<crate::auth::UserRow, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let cookies = leptos_axum::extract::<tower_cookies::Cookies>()
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    crate::auth::require_user(&state.db_pool, &cookies)
        .await
        .map_err(ServerFnError::new)
}

/// Lowercase letters/digits only, runs separated by a single hyphen — a
/// cosmetic URL suffix, not the lookup key (routing is always by page id),
/// so this never needs to be unique: two siblings can end up with the same
/// slug and nothing breaks, it's just a less distinctive link to look at.
#[cfg(feature = "ssr")]
fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_was_hyphen = true; // avoids a leading hyphen
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            slug.push('-');
            last_was_hyphen = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("page");
    }
    slug
}

// Browsing the tree and reading a page is public, same as cards/decks —
// only create/edit/move/delete require a signed-in user.

#[server]
pub async fn list_page_tree() -> Result<Vec<PageTreeNode>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let rows = crate::db::list_page_tree(&state.db_pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|r| PageTreeNode {
            id: r.id,
            parent_id: r.parent_id,
            slug: r.slug,
            title: r.title,
            position: r.position,
        })
        .collect())
}

#[server]
pub async fn search_pages(query: String) -> Result<Vec<PageSearchResult>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let rows = crate::db::search_pages(&state.db_pool, &query)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|r| PageSearchResult {
            id: r.id,
            slug: r.slug,
            title: r.title,
            snippet: r.snippet,
        })
        .collect())
}

#[server]
pub async fn get_page(page_id: Uuid) -> Result<Option<PageContent>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let row = crate::db::get_page(&state.db_pool, page_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(row.map(|r| {
        let content_html = render_blocks_html(&r.content_json);
        let toc = extract_toc(&content_html);
        let content_html = with_table_of_contents(content_html, &toc);
        let content_html = with_footnotes(content_html);
        PageContent {
            id: r.id,
            parent_id: r.parent_id,
            slug: r.slug,
            title: r.title,
            content_json: r.content_json,
            content_markdown: r.content_markdown,
            content_html,
        }
    }))
}

#[server]
pub async fn create_page(parent_id: Option<Uuid>, title: String) -> Result<Uuid, ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    let slug = slugify(&title);
    crate::db::create_page(&state.db_pool, user.id, parent_id, &title, &slug)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn save_page_revision(
    page_id: Uuid,
    title: String,
    content_json: String,
    content_markdown: String,
) -> Result<Uuid, ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::save_page_revision(
        &state.db_pool,
        page_id,
        user.id,
        &title,
        &content_json,
        &content_markdown,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn rename_page(page_id: Uuid, title: String) -> Result<(), ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    let slug = slugify(&title);
    crate::db::rename_page(&state.db_pool, page_id, &title, &slug)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Reparents to the end of the new parent's children — not an arbitrary
/// drag-to-position reorder (no UI for that yet), so the position is
/// always computed server-side rather than trusted from the client. Cycle
/// prevention (moving a page into its own subtree) is the caller's job:
/// the "Move" picker in app.rs excludes the page itself and its
/// descendants from the choices it offers, rather than this function
/// re-deriving the same tree walk the client already did to build that
/// list.
#[server]
pub async fn move_page(page_id: Uuid, new_parent_id: Option<Uuid>) -> Result<(), ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::move_page_to_end(&state.db_pool, page_id, new_parent_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn delete_page(page_id: Uuid) -> Result<(), ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    let has_children = crate::db::page_has_children(&state.db_pool, page_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if has_children {
        return Err(ServerFnError::new(
            "move or delete this page's children first",
        ));
    }
    crate::db::delete_page(&state.db_pool, page_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn list_page_revisions(page_id: Uuid) -> Result<Vec<PageRevisionSummary>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let rows = crate::db::list_page_revisions(&state.db_pool, page_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    rows.into_iter()
        .map(|r| {
            let created_at = r
                .created_at
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|e| ServerFnError::new(e.to_string()))?;
            Ok(PageRevisionSummary {
                id: r.id,
                author_id: r.author_id,
                title: r.title,
                created_at,
            })
        })
        .collect()
}

/// Diffs `revision_id` against the revision immediately before it (a
/// GitHub-commit-style "what changed here", not an arbitrary A/B compare —
/// see the wiki planning notes for why that's the deliberate starting scope).
#[server]
pub async fn get_revision_diff(
    page_id: Uuid,
    revision_id: Uuid,
) -> Result<RevisionDiff, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let revision = crate::db::get_revision(&state.db_pool, revision_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("revision not found"))?;
    let previous_markdown =
        crate::db::previous_revision_markdown(&state.db_pool, page_id, revision_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    let diff_html = render_diff_html(
        previous_markdown.as_deref().unwrap_or(""),
        &revision.content_markdown,
    );
    Ok(RevisionDiff {
        current_title: revision.title,
        diff_html,
    })
}

/// Attaches a grammar page to a card — the card side of the link
/// (`add_card_to_deck`'s counterpart) dispatches `RemovePageFromCard` the
/// same way, from `EditCardPage`'s "Grammar pages" checkbox list.
#[server]
pub async fn add_page_to_card(card_id: Uuid, page_id: Uuid) -> Result<(), ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::add_page_to_card(&state.db_pool, card_id, page_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn remove_page_from_card(card_id: Uuid, page_id: Uuid) -> Result<(), ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::remove_page_from_card(&state.db_pool, card_id, page_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// The cards that link to `page_id` — rendered as a backlink list on the
/// page itself, so the connection a card makes to this page is visible
/// from both sides, not just from the card's edit form.
#[server]
pub async fn cards_for_page(page_id: Uuid) -> Result<Vec<LinkedCard>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let rows = crate::db::cards_for_page(&state.db_pool, page_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|r| LinkedCard {
            id: r.id,
            target_text: r.target_text,
        })
        .collect())
}

#[cfg(feature = "ssr")]
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `escape_html` plus the one character that matters for a double-quoted
/// attribute value specifically — a bare `escape_html` would let a `"` in,
/// say, an image's alt text or a link's `href` close the attribute early
/// and inject whatever follows as new markup/attributes.
#[cfg(feature = "ssr")]
fn escape_attr(s: &str) -> String {
    escape_html(s).replace('"', "&quot;")
}

/// Line-level diff, not word-level: markdown source is already closer to
/// prose-per-line than code, and a line granularity is both cheaper to
/// compute and plainer to read than `similar`'s finer-grained options here.
#[cfg(feature = "ssr")]
fn render_diff_html(old: &str, new: &str) -> String {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::from_lines(old, new);
    let mut html = String::new();
    for change in diff.iter_all_changes() {
        let escaped = escape_html(&change.to_string());
        match change.tag() {
            ChangeTag::Delete => {
                html.push_str("<del>");
                html.push_str(&escaped);
                html.push_str("</del>");
            }
            ChangeTag::Insert => {
                html.push_str("<ins>");
                html.push_str(&escaped);
                html.push_str("</ins>");
            }
            ChangeTag::Equal => html.push_str(&escaped),
        }
    }
    html
}

/// Never rewrites history — restoring an old revision creates a *new* one
/// with that old content, same append-only shape as everything else here.
#[server]
pub async fn restore_revision(page_id: Uuid, revision_id: Uuid) -> Result<Uuid, ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    let revision = crate::db::get_revision(&state.db_pool, revision_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("revision not found"))?;
    crate::db::save_page_revision(
        &state.db_pool,
        page_id,
        user.id,
        &revision.title,
        &revision.content_json,
        &revision.content_markdown,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Server-renders `content_json` — the canonical, live `BlockNote`
/// document, not the lossy `content_markdown` export — to HTML for a
/// read-only view, no editor JS shipped to someone who's just reading.
/// `content_markdown` round-trips text color, highlight color, and text
/// alignment through nothing at all (`BlockNote`'s own markdown exporter
/// strips color spans outright and has no markdown syntax for alignment),
/// so rendering from `content_json` directly is what makes those three
/// toolbar controls actually mean something once a page is saved.
///
/// Every heading this emits lands on its own line, as exactly
/// `<hN id="...">...</hN>` with nothing else sharing that line — not an
/// HTML-formatting nicety, but the one fixed shape `extract_toc` below
/// scans for (a convention inherited from when this came from comrak's own
/// output, kept because `extract_toc` still depends on it).
#[cfg(feature = "ssr")]
fn render_blocks_html(content_json: &str) -> String {
    let Ok(Value::Array(blocks)) = serde_json::from_str::<Value>(content_json) else {
        // Covers a brand-new page's `"{}"` (see db.rs's create_page) along
        // with any other non-array content — nothing to render rather than
        // a server error over what's just an empty document.
        return String::new();
    };
    let mut heading_ids = HashMap::new();
    let mut footnote_count = 0u32;
    render_block_sequence(&blocks, &mut heading_ids, &mut footnote_count)
}

/// Renders a flat array of sibling blocks — the document root, or any
/// block's `children` — grouping consecutive same-type list items into one
/// `<ul>`/`<ol>` the way `BlockNote` represents a list: as adjacent
/// top-level blocks, not a dedicated list-container block type.
#[cfg(feature = "ssr")]
fn render_block_sequence(
    blocks: &[Value],
    heading_ids: &mut HashMap<String, u32>,
    footnote_count: &mut u32,
) -> String {
    let mut html = String::new();
    let mut i = 0;
    while i < blocks.len() {
        let block_type = blocks[i].get("type").and_then(Value::as_str).unwrap_or("");
        if let Some(wrapper) = list_wrapper(block_type) {
            let mut j = i + 1;
            while j < blocks.len()
                && blocks[j].get("type").and_then(Value::as_str) == Some(block_type)
            {
                j += 1;
            }
            html.push_str(&render_list(
                wrapper,
                block_type,
                &blocks[i..j],
                heading_ids,
                footnote_count,
            ));
            i = j;
        } else {
            html.push_str(&render_single_block(
                &blocks[i],
                heading_ids,
                footnote_count,
            ));
            html.push('\n');
            i += 1;
        }
    }
    html
}

#[cfg(feature = "ssr")]
#[derive(Clone, Copy)]
struct ListWrapper {
    tag: &'static str,
    class: Option<&'static str>,
}

#[cfg(feature = "ssr")]
fn list_wrapper(block_type: &str) -> Option<ListWrapper> {
    match block_type {
        "bulletListItem" => Some(ListWrapper {
            tag: "ul",
            class: None,
        }),
        "numberedListItem" => Some(ListWrapper {
            tag: "ol",
            class: None,
        }),
        "checkListItem" => Some(ListWrapper {
            tag: "ul",
            class: Some("wiki-checklist"),
        }),
        "toggleListItem" => Some(ListWrapper {
            tag: "ul",
            class: Some("wiki-togglelist"),
        }),
        _ => None,
    }
}

#[cfg(feature = "ssr")]
fn render_list(
    wrapper: ListWrapper,
    block_type: &str,
    items: &[Value],
    heading_ids: &mut HashMap<String, u32>,
    footnote_count: &mut u32,
) -> String {
    let mut html = format!("\n<{}", wrapper.tag);
    if let Some(class) = wrapper.class {
        let _ = write!(html, " class=\"{class}\"");
    }
    // Only a numbered list's first item ever carries `start` — BlockNote
    // only needs it there, since every following item just counts on.
    if wrapper.tag == "ol"
        && let Some(start) = items[0]
            .get("props")
            .and_then(|p| p.get("start"))
            .and_then(Value::as_u64)
        && start != 1
    {
        let _ = write!(html, " start=\"{start}\"");
    }
    html.push('>');
    for item in items {
        html.push_str(&render_list_item(
            block_type,
            item,
            heading_ids,
            footnote_count,
        ));
    }
    let _ = writeln!(html, "</{}>", wrapper.tag);
    html
}

#[cfg(feature = "ssr")]
fn render_list_item(
    block_type: &str,
    item: &Value,
    heading_ids: &mut HashMap<String, u32>,
    footnote_count: &mut u32,
) -> String {
    let props = item.get("props").unwrap_or(&Value::Null);
    let content = item.get("content").unwrap_or(&Value::Null);
    let children: &[Value] = item
        .get("children")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let inline = render_inline(content, footnote_count);
    let children_html = render_block_sequence(children, heading_ids, footnote_count);
    let style = style_attr(props);
    match block_type {
        "checkListItem" => {
            let checked = props
                .get("checked")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let checked_attr = if checked { " checked" } else { "" };
            format!(
                "<li{style}><label><input type=\"checkbox\" disabled{checked_attr}/> {inline}</label>{children_html}</li>"
            )
        }
        "toggleListItem" if !children_html.trim().is_empty() => {
            format!(
                "<li{style}><details open><summary>{inline}</summary>\n{children_html}</details></li>"
            )
        }
        _ => format!("<li{style}>{inline}{children_html}</li>"),
    }
}

/// Everything that isn't a list item — rendered on its own, never grouped
/// with a sibling the way list items are.
#[cfg(feature = "ssr")]
fn render_single_block(
    block: &Value,
    heading_ids: &mut HashMap<String, u32>,
    footnote_count: &mut u32,
) -> String {
    let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
    let props = block.get("props").unwrap_or(&Value::Null);
    let content = block.get("content").unwrap_or(&Value::Null);
    let children: &[Value] = block
        .get("children")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let children_html = render_block_sequence(children, heading_ids, footnote_count);
    let style = style_attr(props);
    match block_type {
        "heading" => render_heading(
            props,
            content,
            &children_html,
            &style,
            heading_ids,
            footnote_count,
        ),
        "quote" => format!(
            "<blockquote{style}>{}</blockquote>{children_html}",
            render_inline(content, footnote_count)
        ),
        "codeBlock" => {
            let language = props.get("language").and_then(Value::as_str).unwrap_or("");
            let class = if language.is_empty() {
                String::new()
            } else {
                format!(" class=\"language-{}\"", escape_attr(language))
            };
            format!(
                "<pre><code{class}>{}</code></pre>{children_html}",
                escape_html(&inline_plain_text(content))
            )
        }
        "divider" => format!("<hr/>{children_html}"),
        "image" => render_media_block(MediaKind::Image, props, &children_html),
        "video" => render_media_block(MediaKind::Video, props, &children_html),
        "audio" => render_media_block(MediaKind::Audio, props, &children_html),
        "file" => render_media_block(MediaKind::File, props, &children_html),
        "table" => format!("{}{children_html}", render_table(content, footnote_count)),
        // `@blocknote/xl-multi-column`'s two block types (see
        // editor/src/main.tsx's `wikiSchema`) — a `columnList` is content
        // "none" and holds nothing but `column` children side by side; each
        // `column`'s own content is in turn its `children`, not `content`
        // (also "none" — a column is purely a layout container).
        "columnList" => format!("\n<div class=\"wiki-columns\">{children_html}</div>\n"),
        "column" => render_column(props, &children_html),
        // A placeholder widget in the editor (see editor/src/main.tsx's
        // TableOfContentsBlock) — this fixed marker is what get_page's
        // with_table_of_contents splices the real, rendered box into.
        "tableOfContents" => TOC_MARKER_HTML.to_string(),
        // "paragraph", and anything this renderer doesn't recognize yet
        // (forward compatibility with a future BlockNote block type) —
        // falls back to a plain paragraph rather than silently dropping
        // its text.
        _ => format!(
            "<p{style}>{}</p>{children_html}",
            render_inline(content, footnote_count)
        ),
    }
}

#[cfg(feature = "ssr")]
fn render_heading(
    props: &Value,
    content: &Value,
    children_html: &str,
    style: &str,
    heading_ids: &mut HashMap<String, u32>,
    footnote_count: &mut u32,
) -> String {
    let level = props
        .get("level")
        .and_then(Value::as_u64)
        .and_then(|n| u8::try_from(n).ok())
        .map_or(3, |n| n.clamp(1, 6));
    let id = unique_heading_id(&inline_plain_text(content), heading_ids);
    let tag = format!(
        "<h{level} id=\"{id}\"{style}>{}</h{level}>",
        render_inline(content, footnote_count)
    );
    let toggleable = props
        .get("isToggleable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if toggleable {
        format!(
            "\n<details open class=\"wiki-toggle\">\n<summary>\n{tag}\n</summary>\n{children_html}</details>\n"
        )
    } else {
        format!("\n{tag}\n{children_html}")
    }
}

#[cfg(feature = "ssr")]
#[derive(Clone, Copy)]
enum MediaKind {
    Image,
    Video,
    Audio,
    File,
}

#[cfg(feature = "ssr")]
fn render_media_block(kind: MediaKind, props: &Value, children_html: &str) -> String {
    let url = props.get("url").and_then(Value::as_str).unwrap_or("");
    if url.is_empty() {
        // Nothing uploaded yet (a media block inserted but not filled in) —
        // nothing to show rather than a broken `<img src="">`.
        return children_html.to_string();
    }
    let name = props.get("name").and_then(Value::as_str).unwrap_or("");
    let caption = props.get("caption").and_then(Value::as_str).unwrap_or("");
    // File has no `showPreview` prop at all (it's always just a link) —
    // `unwrap_or` with File's own "always false" default covers that
    // absence the same way a missing key on any other prop does.
    let show_preview = props
        .get("showPreview")
        .and_then(Value::as_bool)
        .unwrap_or(!matches!(kind, MediaKind::File));
    let href = escape_attr(url);
    let media = match kind {
        MediaKind::Image if show_preview => {
            let width = props
                .get("previewWidth")
                .and_then(Value::as_u64)
                .map_or(String::new(), |w| format!(" width=\"{w}\""));
            format!("<img src=\"{href}\" alt=\"{}\"{width}/>", escape_attr(name))
        }
        MediaKind::Video if show_preview => {
            let width = props
                .get("previewWidth")
                .and_then(Value::as_u64)
                .map_or(String::new(), |w| format!(" width=\"{w}\""));
            format!("<video src=\"{href}\" controls{width}></video>")
        }
        MediaKind::Audio if show_preview => format!("<audio src=\"{href}\" controls></audio>"),
        _ => {
            let label = if name.is_empty() { url } else { name };
            format!("<a href=\"{href}\">{}</a>", escape_html(label))
        }
    };
    let figure = if caption.is_empty() {
        media
    } else {
        format!(
            "<figure>{media}<figcaption>{}</figcaption></figure>",
            escape_html(caption)
        )
    };
    format!("\n{figure}\n{children_html}")
}

/// A column's `width` is a relative ratio against its sibling columns
/// (`flex-grow`'s own native meaning), not a percentage — a two-column
/// `columnList` with widths `1`/`1` splits evenly, `1`/`2` gives the second
/// column twice the first's share. Validated before it ever reaches a
/// `style` attribute, same reasoning as `safe_color`: a number straight out
/// of `content_json` could be `NaN`/`Infinity`/negative, none of which
/// `flex` handles sensibly.
#[cfg(feature = "ssr")]
fn render_column(props: &Value, children_html: &str) -> String {
    let width = props
        .get("width")
        .and_then(Value::as_f64)
        .filter(|w| w.is_finite() && *w > 0.0)
        .unwrap_or(1.0);
    format!("<div class=\"wiki-column\" style=\"flex:{width}\">{children_html}</div>")
}

/// A table's `content` is one object (`{"type": "tableContent", "rows":
/// [...]}`), not an array like every other block's `content` — its own
/// shape, carried over unchanged from `BlockNote`'s document model.
#[cfg(feature = "ssr")]
fn render_table(content: &Value, footnote_count: &mut u32) -> String {
    let Some(rows) = content.get("rows").and_then(Value::as_array) else {
        return String::new();
    };
    let mut html = String::from("\n<table>\n<tbody>\n");
    for row in rows {
        html.push_str("<tr>");
        if let Some(cells) = row.get("cells").and_then(Value::as_array) {
            for cell in cells {
                html.push_str(&render_table_cell(cell, footnote_count));
            }
        }
        html.push_str("</tr>\n");
    }
    html.push_str("</tbody>\n</table>\n");
    html
}

#[cfg(feature = "ssr")]
fn render_table_cell(cell: &Value, footnote_count: &mut u32) -> String {
    let props = cell.get("props").unwrap_or(&Value::Null);
    // A cell's own `content` is a plain inline-content array directly —
    // unlike a table BLOCK's `content` above, a cell isn't wrapped in its
    // own `{"type": ...}` object.
    let content = cell.get("content").unwrap_or(&Value::Null);
    let mut attrs = style_attr(props);
    if let Some(colspan) = props.get("colspan").and_then(Value::as_u64)
        && colspan > 1
    {
        let _ = write!(attrs, " colspan=\"{colspan}\"");
    }
    if let Some(rowspan) = props.get("rowspan").and_then(Value::as_u64)
        && rowspan > 1
    {
        let _ = write!(attrs, " rowspan=\"{rowspan}\"");
    }
    format!("<td{attrs}>{}</td>", render_inline(content, footnote_count))
}

/// Inline content — the `text`/`link`/`footnoteRef` item array shared by
/// every block whose `content` holds actual prose (paragraph, heading,
/// quote, list items, table cells).
#[cfg(feature = "ssr")]
fn render_inline(content: &Value, footnote_count: &mut u32) -> String {
    let Some(items) = content.as_array() else {
        return String::new();
    };
    let mut html = String::new();
    for item in items {
        html.push_str(&render_inline_item(item, footnote_count));
    }
    html
}

#[cfg(feature = "ssr")]
fn render_inline_item(item: &Value, footnote_count: &mut u32) -> String {
    match item.get("type").and_then(Value::as_str) {
        Some("link") => {
            let href = item.get("href").and_then(Value::as_str).unwrap_or("");
            let inner = render_inline(item.get("content").unwrap_or(&Value::Null), footnote_count);
            return format!("<a href=\"{}\">{inner}</a>", escape_attr(href));
        }
        // editor/src/main.tsx's FootnoteRef — a plain-text annotation (see
        // its own doc comment on why `content: "none"` rather than nested
        // rich content), so its text lives directly in `props.text` here,
        // not walked as further inline content the way `link`'s above is.
        // Numbered purely by the order encountered walking the saved
        // content, same strategy `unique_heading_id`/`heading_ids` already
        // uses for heading ids — not `props.noteId`, which only the live
        // editor's own numbering (editor/src/main.tsx's footnoteNumber)
        // ever reads. extract_footnotes below pulls this same text back out
        // of `data-footnote-text` to build the generated list at the
        // article's end, the same after-the-fact-scan `extract_toc` uses
        // for the "Contents" box, rather than threading a second
        // accumulator alongside this counter.
        Some("footnoteRef") => {
            *footnote_count += 1;
            let number = *footnote_count;
            let text = item
                .get("props")
                .and_then(|p| p.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            return format!(
                "<sup class=\"wiki-footnote-ref\" id=\"fnref-{number}\" data-footnote-text=\"{}\"><a href=\"#fn-{number}\">{number}</a></sup>",
                escape_attr(text),
            );
        }
        _ => {}
    }
    let text = escape_html(item.get("text").and_then(Value::as_str).unwrap_or(""));
    wrap_styles(text, item.get("styles"))
}

/// Applies one inline-content item's `styles` (bold/italic/underline/
/// strike/code, plus text/highlight color) to its already-escaped text.
/// Nesting order only matters for validity, not appearance — a `<span
/// style>` wrapping a `<strong>` looks identical to the reverse — so this
/// just wraps outward from the text in a fixed order, innermost mark first.
#[cfg(feature = "ssr")]
fn wrap_styles(text: String, styles: Option<&Value>) -> String {
    let Some(styles) = styles else {
        return text;
    };
    let mut html = text;
    if styles.get("code").and_then(Value::as_bool).unwrap_or(false) {
        html = format!("<code>{html}</code>");
    }
    if styles
        .get("strike")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        html = format!("<s>{html}</s>");
    }
    if styles
        .get("underline")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        html = format!("<u>{html}</u>");
    }
    if styles
        .get("italic")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        html = format!("<em>{html}</em>");
    }
    if styles.get("bold").and_then(Value::as_bool).unwrap_or(false) {
        html = format!("<strong>{html}</strong>");
    }
    let decls = color_decls(styles);
    if !decls.is_empty() {
        html = format!("<span style=\"{}\">{html}</span>", decls.join(";"));
    }
    html
}

/// A block's own `props` carry the same two color keys as an inline
/// style's `styles`, plus `textAlignment` (a block-only concept — there's
/// no such thing as mid-paragraph alignment) — this is the block-level
/// counterpart to `wrap_styles`, producing one `style="..."` attribute
/// (with its leading space) rather than wrapping markup.
#[cfg(feature = "ssr")]
fn style_attr(props: &Value) -> String {
    let mut decls = color_decls(props);
    if let Some(align) = props.get("textAlignment").and_then(Value::as_str)
        && matches!(align, "center" | "right" | "justify")
    {
        decls.push(format!("text-align:{align}"));
    }
    if decls.is_empty() {
        String::new()
    } else {
        format!(" style=\"{}\"", decls.join(";"))
    }
}

#[cfg(feature = "ssr")]
fn color_decls(value: &Value) -> Vec<String> {
    let mut decls = Vec::new();
    if let Some(color) = value.get("textColor").and_then(Value::as_str)
        && let Some(css) = safe_color(color, named_text_color)
    {
        decls.push(format!("color:{css}"));
    }
    if let Some(color) = value.get("backgroundColor").and_then(Value::as_str)
        && let Some(css) = safe_color(color, named_background_color)
    {
        decls.push(format!("background-color:{css}"));
    }
    decls
}

/// Only ever resolves to one of the 9 swatch hexes below or a validated
/// `#rgb`/`#rgba`/`#rrggbb`/`#rrggbbaa` literal — never the raw input
/// string. `content_json` isn't validated on the way in (`save_page_revision`
/// stores whatever the client sends), so a crafted value here is reachable
/// by anyone who can save a page; passing it straight into a `style="..."`
/// attribute would let it break out of the attribute via a literal `"`.
/// Anything that isn't one of those two shapes is dropped — no color
/// applied — rather than guessed at.
#[cfg(feature = "ssr")]
fn safe_color(value: &str, named: impl Fn(&str) -> Option<&'static str>) -> Option<String> {
    if value == "default" {
        None
    } else if let Some(hex) = named(value) {
        Some(hex.to_string())
    } else if is_hex_color(value) {
        Some(value.to_string())
    } else {
        None
    }
}

#[cfg(feature = "ssr")]
fn is_hex_color(value: &str) -> bool {
    value.strip_prefix('#').is_some_and(|hex| {
        matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// `BlockNote`'s 9-swatch Notion-style palette (`COLORS_DEFAULT` in its own
/// source) — the light-mode text shade of each name; `named_background_color`
/// below is the same 9 names' background shade, a different hex per name,
/// not just a lighter version of the same one.
#[cfg(feature = "ssr")]
fn named_text_color(name: &str) -> Option<&'static str> {
    match name {
        "gray" => Some("#9b9a97"),
        "brown" => Some("#64473a"),
        "red" => Some("#e03e3e"),
        "orange" => Some("#d9730d"),
        "yellow" => Some("#dfab01"),
        "green" => Some("#4d6461"),
        "blue" => Some("#0b6e99"),
        "purple" => Some("#6940a5"),
        "pink" => Some("#ad1a72"),
        _ => None,
    }
}

#[cfg(feature = "ssr")]
fn named_background_color(name: &str) -> Option<&'static str> {
    match name {
        "gray" => Some("#ebeced"),
        "brown" => Some("#e9e5e3"),
        "red" => Some("#fbe4e4"),
        "orange" => Some("#f6e9d9"),
        "yellow" => Some("#fbf3db"),
        "green" => Some("#ddedea"),
        "blue" => Some("#ddebf1"),
        "purple" => Some("#eae4f2"),
        "pink" => Some("#f4dfeb"),
        _ => None,
    }
}

/// The plain text of an inline-content array, with no markup at all — just
/// for deriving a heading's `id` slug, where "A `code` word"'s id should be
/// built from "A code word", not from text still carrying its own HTML.
#[cfg(feature = "ssr")]
fn inline_plain_text(content: &Value) -> String {
    let Some(items) = content.as_array() else {
        return String::new();
    };
    let mut text = String::new();
    for item in items {
        if item.get("type").and_then(Value::as_str) == Some("link") {
            text.push_str(&inline_plain_text(
                item.get("content").unwrap_or(&Value::Null),
            ));
        } else {
            text.push_str(item.get("text").and_then(Value::as_str).unwrap_or(""));
        }
    }
    text
}

/// Reuses the page-slug algorithm for heading ids too (lowercase
/// alphanumeric runs, hyphen-joined) — then de-duplicates with a numeric
/// suffix, since two headings with the same text is common (e.g. two
/// sections both titled "Examples") and would otherwise collide on the
/// same in-page anchor.
#[cfg(feature = "ssr")]
fn unique_heading_id(text: &str, used: &mut HashMap<String, u32>) -> String {
    let base = slugify(text);
    let count = used.entry(base.clone()).or_insert(0);
    *count += 1;
    if *count == 1 {
        base
    } else {
        format!("{base}-{}", *count - 1)
    }
}

/// Pulls the H1-H3 headings `render_blocks_html` just gave real `id`
/// attributes out of its own output, rather than threading a parallel
/// `Vec<TocEntry>` back out of that whole recursive render by hand. A small
/// hand-rolled scan rather than a full HTML parser dependency:
/// `render_blocks_html` always emits one `<hN id="...">...</hN>` per line
/// (see its own doc comment), so this only needs to understand that one
/// fixed shape, not arbitrary HTML.
#[cfg(feature = "ssr")]
fn extract_toc(html: &str) -> Vec<TocEntry> {
    let mut entries = Vec::new();
    for line in html.lines() {
        for level in 1u8..=3 {
            let open_tag = format!("<h{level} id=\"");
            let Some(rest) = line.strip_prefix(&open_tag) else {
                continue;
            };
            let Some((id, rest)) = rest.split_once('"') else {
                break;
            };
            // `id` isn't necessarily the tag's last attribute — a centered
            // or colored heading also carries a `style="..."` from
            // `style_attr`, landing between `id="..."` and the tag's own
            // closing `>` (confirmed live: a centered heading's `>` was
            // nowhere near where this used to look for it, silently
            // dropping the heading from the TOC — not just a missing-id
            // edge case). None of `style_attr`'s values can themselves
            // contain `>` (they're built from validated hex/named-color/
            // fixed-keyword fragments only), so the next `>` in the line is
            // always the real end of the opening tag.
            let Some(tag_close) = rest.find('>') else {
                break;
            };
            let rest = &rest[tag_close + 1..];
            // The heading's own text ends where comrak's self-link anchor
            // (`header_ids`'s `<a href="#...">...</a>`) or the closing tag
            // starts, whichever comes first — a heading with no inline
            // markup has no closing `</hN>` before that anchor, so both
            // ends must be checked.
            let close_tag = format!("</h{level}>");
            let text_end = rest
                .find("<a href=\"#")
                .or_else(|| rest.find(&close_tag))
                .unwrap_or(rest.len());
            let text = strip_tags(&rest[..text_end]);
            if !text.trim().is_empty() {
                entries.push(TocEntry {
                    level,
                    id: id.to_string(),
                    text,
                });
            }
            break;
        }
    }
    entries
}

/// `render_single_block` emits exactly this for a `tableOfContents` block
/// (see `editor/src/main.tsx`'s `TableOfContentsBlock`) — a fixed, inert
/// marker rather than anything a user could type, which `with_table_of_contents`
/// below replaces with the real, rendered "Contents" box. Kept in sync by
/// convention (there's no shared build between the Rust and TypeScript
/// sides to enforce it), not by import.
#[cfg(feature = "ssr")]
const TOC_MARKER_HTML: &str = "<div data-wiki-toc-marker=\"\"></div>";

/// Before the `tableOfContents` block type existed, a plain paragraph
/// containing the literal text "[TOC]" was the marker instead — this is
/// what `render_single_block`'s fallback paragraph arm renders that one
/// block as. `editor/src/main.tsx`'s `upgradeLegacyTocMarkers` converts it
/// to the real block type the next time a page is opened for editing (and
/// autosaves that upgrade), but a page that hasn't been reopened since
/// still has this in its stored `content_json`, so the read view needs to
/// keep recognizing it too rather than showing it as literal text.
#[cfg(feature = "ssr")]
const LEGACY_TOC_MARKER_HTML: &str = "<p>[TOC]</p>";

/// Splices a rendered "Contents" box into `html` at its marker (either the
/// current one or the legacy one above), or — if the page has no marker at
/// all, which includes every page's revisions made before this feature was
/// positionable — prepends one automatically when there's enough to
/// navigate. A page with fewer than 2 headings never gets one either way:
/// nothing to navigate between.
#[cfg(feature = "ssr")]
fn with_table_of_contents(html: String, toc: &[TocEntry]) -> String {
    if toc.len() < 2 {
        return html;
    }
    let rendered = toc_html(toc);
    if html.contains(TOC_MARKER_HTML) {
        html.replacen(TOC_MARKER_HTML, &rendered, 1)
    } else if html.contains(LEGACY_TOC_MARKER_HTML) {
        html.replacen(LEGACY_TOC_MARKER_HTML, &rendered, 1)
    } else {
        format!("{rendered}{html}")
    }
}

#[cfg(feature = "ssr")]
fn toc_html(toc: &[TocEntry]) -> String {
    use std::fmt::Write as _;

    let mut html = String::from(
        "<nav class=\"wiki-toc\" aria-label=\"Table of contents\"><p class=\"wiki-toc-heading\">Contents</p><ul>",
    );
    for entry in toc {
        let _ = write!(
            html,
            "<li class=\"wiki-toc-level-{}\"><a href=\"#{}\">{}</a></li>",
            entry.level,
            entry.id,
            escape_html(&entry.text),
        );
    }
    html.push_str("</ul></nav>");
    html
}

/// Pulls each footnote's number and text back out of `render_blocks_html`'s
/// own output — the same after-the-fact-scan strategy `extract_toc` uses
/// for headings, rather than threading a second accumulator (a `Vec`)
/// alongside the plain counter `render_inline_item` already needs just to
/// assign the numbers. Scans for one fixed, self-contained marker shape
/// (`id="fnref-N" data-footnote-text="..."`, both emitted together by
/// `render_inline_item`'s own `footnoteRef` arm) rather than a full HTML
/// parser dependency. `text` comes out still `escape_attr`-escaped exactly
/// as `render_inline_item` wrote it — safe to drop straight into `<li>`
/// text content unchanged, since `escape_attr` escapes a strict superset
/// (`&`/`</>`/`"`) of what plain text content (`&`/`</>`) needs.
#[cfg(feature = "ssr")]
fn extract_footnotes(html: &str) -> Vec<(u32, String)> {
    let mut entries = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("id=\"fnref-") {
        rest = &rest[start + "id=\"fnref-".len()..];
        let Some(id_end) = rest.find('"') else { break };
        let Ok(number) = rest[..id_end].parse::<u32>() else {
            break;
        };
        rest = &rest[id_end..];
        let Some(text_start) = rest.find("data-footnote-text=\"") else {
            break;
        };
        rest = &rest[text_start + "data-footnote-text=\"".len()..];
        let Some(text_end) = rest.find('"') else {
            break;
        };
        entries.push((number, rest[..text_end].to_string()));
        rest = &rest[text_end..];
    }
    entries
}

/// Appends a generated "Footnotes" section listing each footnote `html`'s
/// own inline markers already numbered and linked to — unlike
/// `with_table_of_contents`, there's no marker to splice at: a footnotes
/// list, unlike a table of contents, has no place in the document a writer
/// would naturally position it themselves, so it always lands at the very
/// end. A no-op when the page has none.
#[cfg(feature = "ssr")]
fn with_footnotes(html: String) -> String {
    let footnotes = extract_footnotes(&html);
    if footnotes.is_empty() {
        return html;
    }
    let mut section = String::from("<section class=\"wiki-footnotes\"><hr/><ol>");
    for (number, text) in &footnotes {
        let _ = write!(
            section,
            "<li id=\"fn-{number}\">{text} <a href=\"#fnref-{number}\" class=\"wiki-footnote-backlink\" aria-label=\"Back to reference {number}\">\u{21a9}</a></li>",
        );
    }
    section.push_str("</ol></section>");
    format!("{html}{section}")
}

/// Strips inline HTML tags (e.g. `<code>`/`<strong>` from inline markdown
/// inside a heading) and unescapes the handful of entities `render_markdown`
/// can actually produce in heading text, down to the plain text a TOC
/// label should show.
#[cfg(feature = "ssr")]
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            _ => out.push(ch),
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;
    use serde_json::json;

    fn default_props() -> Value {
        json!({"textColor": "default", "textAlignment": "left", "backgroundColor": "default"})
    }

    fn text_item(text: &str) -> Value {
        json!({"type": "text", "text": text, "styles": {}})
    }

    fn styled_text(text: &str, styles: &Value) -> Value {
        json!({"type": "text", "text": text, "styles": styles})
    }

    fn paragraph(text: &str) -> Value {
        json!({"type": "paragraph", "props": default_props(), "content": [text_item(text)], "children": []})
    }

    fn toc_marker() -> Value {
        json!({"type": "tableOfContents", "props": {}, "content": [], "children": []})
    }

    fn footnote_ref(text: &str) -> Value {
        json!({"type": "footnoteRef", "props": {"noteId": "ignored-by-the-renderer", "text": text}})
    }

    fn heading(level: u64, text: &str) -> Value {
        json!({
            "type": "heading",
            "props": {"level": level, "textColor": "default", "isToggleable": false, "textAlignment": "left", "backgroundColor": "default"},
            "content": [text_item(text)],
            "children": [],
        })
    }

    fn doc(blocks: Vec<Value>) -> String {
        Value::Array(blocks).to_string()
    }

    #[test]
    fn extract_toc_finds_h1_through_h3_in_document_order() {
        let html = render_blocks_html(&doc(vec![
            heading(1, "One"),
            paragraph("intro"),
            heading(2, "Two"),
            heading(3, "Three"),
            heading(4, "Four"),
        ]));
        let toc = extract_toc(&html);
        assert_eq!(
            toc,
            vec![
                TocEntry {
                    level: 1,
                    id: "one".to_string(),
                    text: "One".to_string()
                },
                TocEntry {
                    level: 2,
                    id: "two".to_string(),
                    text: "Two".to_string()
                },
                TocEntry {
                    level: 3,
                    id: "three".to_string(),
                    text: "Three".to_string()
                },
            ]
        );
    }

    #[test]
    fn extract_toc_strips_inline_markup_and_matches_the_rendered_id() {
        let html = render_blocks_html(&doc(vec![json!({
            "type": "heading",
            "props": {"level": 2, "textColor": "default", "isToggleable": false, "textAlignment": "left", "backgroundColor": "default"},
            "content": [text_item("A "), styled_text("code", &json!({"code": true})), text_item(" word")],
            "children": [],
        })]));
        let toc = extract_toc(&html);
        assert_eq!(toc.len(), 1);
        assert_eq!(toc[0].text, "A code word");
        // The id extract_toc read back must be the exact id render_blocks_html
        // put on the heading, so a link to `#{id}` actually lands there.
        assert!(html.contains(&format!("<h2 id=\"{}\"", toc[0].id)));
    }

    #[test]
    fn extract_toc_is_empty_for_a_page_with_no_headings() {
        let html = render_blocks_html(&doc(vec![paragraph(
            "just a paragraph, no headings at all",
        )]));
        assert_eq!(extract_toc(&html), Vec::<TocEntry>::new());
    }

    #[test]
    fn duplicate_heading_text_gets_distinct_ids() {
        let html = render_blocks_html(&doc(vec![heading(1, "Notes"), heading(2, "Notes")]));
        let toc = extract_toc(&html);
        assert_eq!(toc.len(), 2);
        assert_ne!(toc[0].id, toc[1].id);
    }

    // Regression test: a centered (or colored) heading's `id="..."`
    // attribute isn't immediately followed by `>` — `style="..."` lands in
    // between — which used to make extract_toc's parser `break` and
    // silently drop the heading from the TOC entirely. Confirmed live
    // against a real saved page before this was fixed: a two-heading page
    // with one heading centered produced an empty TOC and left the page's
    // tableOfContents marker unreplaced, exactly what `toc.len() < 2` below
    // would do.
    #[test]
    fn a_centered_heading_still_gets_picked_up_by_extract_toc() {
        let centered_heading = json!({
            "type": "heading",
            "props": {"level": 1, "textColor": "default", "isToggleable": false, "textAlignment": "center", "backgroundColor": "default"},
            "content": [text_item("Centered")],
            "children": [],
        });
        let html = render_blocks_html(&doc(vec![centered_heading, heading(2, "Plain")]));
        let toc = extract_toc(&html);
        assert_eq!(toc.len(), 2, "{html}");
        assert_eq!(toc[0].text, "Centered");
        assert_eq!(toc[1].text, "Plain");
    }

    #[test]
    fn with_table_of_contents_replaces_an_explicit_marker_in_place() {
        let html = render_blocks_html(&doc(vec![
            heading(1, "One"),
            toc_marker(),
            paragraph("intro"),
            heading(2, "Two"),
        ]));
        let toc = extract_toc(&html);
        let out = with_table_of_contents(html, &toc);
        assert!(
            !out.contains(TOC_MARKER_HTML),
            "marker should be gone: {out}"
        );
        // The contents box landed where the marker was (between the
        // heading and "intro"), not at the very top of the document.
        let one_pos = out.find("<h1").unwrap();
        let toc_pos = out.find("wiki-toc").unwrap();
        let intro_pos = out.find("intro").unwrap();
        assert!(one_pos < toc_pos && toc_pos < intro_pos);
    }

    // Regression test: a page saved before the tableOfContents block type
    // existed still has the old literal "[TOC]" paragraph in its stored
    // content_json (editor/src/main.tsx only upgrades it the next time the
    // page is reopened for editing) — the read view must still splice the
    // real box in at that position rather than showing "[TOC]" as text.
    #[test]
    fn with_table_of_contents_replaces_the_legacy_text_marker_in_place() {
        let html = render_blocks_html(&doc(vec![
            heading(1, "One"),
            paragraph("[TOC]"),
            paragraph("intro"),
            heading(2, "Two"),
        ]));
        let toc = extract_toc(&html);
        let out = with_table_of_contents(html, &toc);
        assert!(!out.contains("[TOC]"), "marker should be gone: {out}");
        let one_pos = out.find("<h1").unwrap();
        let toc_pos = out.find("wiki-toc").unwrap();
        let intro_pos = out.find("intro").unwrap();
        assert!(one_pos < toc_pos && toc_pos < intro_pos);
    }

    #[test]
    fn with_table_of_contents_prepends_when_no_marker_is_present() {
        let html = render_blocks_html(&doc(vec![heading(1, "One"), heading(2, "Two")]));
        let toc = extract_toc(&html);
        let out = with_table_of_contents(html, &toc);
        assert!(out.find("wiki-toc").unwrap() < out.find("<h1").unwrap());
    }

    #[test]
    fn with_table_of_contents_does_nothing_below_two_headings() {
        let html = render_blocks_html(&doc(vec![
            heading(1, "Only One"),
            toc_marker(),
            paragraph("text"),
        ]));
        let toc = extract_toc(&html);
        let out = with_table_of_contents(html.clone(), &toc);
        // The marker is left in place rather than silently eaten — a
        // one-heading page has nothing worth navigating, but discarding
        // the marker entirely would look like data loss if a second
        // heading is added later and then removed again.
        assert_eq!(out, html);
    }

    #[test]
    fn renders_bold_italic_underline_strike_and_inline_code_marks() {
        let html = render_blocks_html(&doc(vec![json!({
            "type": "paragraph",
            "props": default_props(),
            "content": [
                styled_text("bold", &json!({"bold": true})),
                styled_text("italic", &json!({"italic": true})),
                styled_text("underline", &json!({"underline": true})),
                styled_text("strike", &json!({"strike": true})),
                styled_text("code", &json!({"code": true})),
            ],
            "children": [],
        })]));
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(html.contains("<em>italic</em>"), "{html}");
        assert!(html.contains("<u>underline</u>"), "{html}");
        assert!(html.contains("<s>strike</s>"), "{html}");
        assert!(html.contains("<code>code</code>"), "{html}");
    }

    #[test]
    fn renders_text_color_and_background_color_as_an_inline_style() {
        let html = render_blocks_html(&doc(vec![json!({
            "type": "paragraph",
            "props": default_props(),
            "content": [styled_text("hi", &json!({"textColor": "red", "backgroundColor": "blue"}))],
            "children": [],
        })]));
        assert!(
            html.contains("style=\"color:#e03e3e;background-color:#ddebf1\""),
            "{html}"
        );
    }

    #[test]
    fn renders_paragraph_text_alignment() {
        let html = render_blocks_html(&doc(vec![json!({
            "type": "paragraph",
            "props": {"textColor": "default", "textAlignment": "center", "backgroundColor": "default"},
            "content": [text_item("hi")],
            "children": [],
        })]));
        assert!(
            html.contains("<p style=\"text-align:center\">hi</p>"),
            "{html}"
        );
    }

    #[test]
    fn left_alignment_is_the_default_and_emits_no_style_attribute() {
        let html = render_blocks_html(&doc(vec![paragraph("hi")]));
        assert!(html.contains("<p>hi</p>"), "{html}");
    }

    #[test]
    fn renders_a_bullet_list_with_a_nested_sublist() {
        let sub_item = json!({
            "type": "bulletListItem",
            "props": default_props(),
            "content": [text_item("Sub")],
            "children": [],
        });
        let item_with_child = json!({
            "type": "bulletListItem",
            "props": default_props(),
            "content": [text_item("A")],
            "children": [sub_item],
        });
        let item_b = json!({
            "type": "bulletListItem",
            "props": default_props(),
            "content": [text_item("B")],
            "children": [],
        });
        let html = render_blocks_html(&doc(vec![item_with_child, item_b]));
        // The nested <ul> must land inside A's <li> (sub-list), not as a
        // sibling after it — checked by relative position rather than an
        // exact string match, since the renderer is free to add whitespace
        // between tags.
        let a_pos = html.find("<li>A").unwrap();
        let sub_pos = html.find("<li>Sub</li>").unwrap();
        let b_pos = html.find("<li>B</li>").unwrap();
        assert!(a_pos < sub_pos && sub_pos < b_pos, "{html}");
    }

    #[test]
    fn renders_a_numbered_list_with_a_custom_start() {
        let first = json!({
            "type": "numberedListItem",
            "props": {"textColor": "default", "textAlignment": "left", "backgroundColor": "default", "start": 5},
            "content": [text_item("Fifth")],
            "children": [],
        });
        let html = render_blocks_html(&doc(vec![first]));
        assert!(html.contains("<ol start=\"5\">"), "{html}");
    }

    #[test]
    fn renders_a_checklist_items_checked_state() {
        let checked = json!({
            "type": "checkListItem",
            "props": {"textColor": "default", "textAlignment": "left", "backgroundColor": "default", "checked": true},
            "content": [text_item("Done")],
            "children": [],
        });
        let unchecked = json!({
            "type": "checkListItem",
            "props": {"textColor": "default", "textAlignment": "left", "backgroundColor": "default", "checked": false},
            "content": [text_item("Not done")],
            "children": [],
        });
        let html = render_blocks_html(&doc(vec![checked, unchecked]));
        assert!(
            html.contains("<input type=\"checkbox\" disabled checked/> Done"),
            "{html}"
        );
        assert!(
            html.contains("<input type=\"checkbox\" disabled/> Not done"),
            "{html}"
        );
    }

    #[test]
    fn renders_a_toggle_heading_with_children_inside_a_details_element() {
        let toggle_heading = json!({
            "type": "heading",
            "props": {"level": 2, "textColor": "default", "isToggleable": true, "textAlignment": "left", "backgroundColor": "default"},
            "content": [text_item("Section")],
            "children": [paragraph("Hidden until opened")],
        });
        let html = render_blocks_html(&doc(vec![toggle_heading]));
        let details_pos = html.find("<details").expect("details element");
        let heading_pos = html.find("<h2").expect("heading");
        let summary_close = html.find("</summary>").expect("summary close");
        let hidden_pos = html.find("Hidden until opened").expect("child content");
        assert!(
            details_pos < heading_pos && heading_pos < summary_close && summary_close < hidden_pos,
            "{html}"
        );
    }

    #[test]
    fn a_non_toggle_heading_renders_with_no_details_wrapper() {
        let html = render_blocks_html(&doc(vec![heading(1, "Plain")]));
        assert!(!html.contains("<details"), "{html}");
        assert!(html.contains("<h1 id=\"plain\">Plain</h1>"), "{html}");
    }

    #[test]
    fn renders_a_table_with_per_cell_alignment() {
        let cell = json!({
            "type": "tableCell",
            "props": {"colspan": 1, "rowspan": 1, "textColor": "default", "textAlignment": "right", "backgroundColor": "default"},
            "content": [text_item("R")],
        });
        let table = json!({
            "type": "table",
            "props": {"textColor": "default"},
            "content": {"type": "tableContent", "rows": [{"cells": [cell]}], "columnWidths": [null]},
            "children": [],
        });
        let html = render_blocks_html(&doc(vec![table]));
        assert!(html.contains("<table>"), "{html}");
        assert!(
            html.contains("<td style=\"text-align:right\">R</td>"),
            "{html}"
        );
    }

    #[test]
    fn renders_an_image_block_with_a_caption() {
        let image = json!({
            "type": "image",
            "props": {
                "textAlignment": "left", "backgroundColor": "default",
                "name": "cat.png", "url": "https://example.com/cat.png",
                "caption": "A cat", "showPreview": true, "previewWidth": 400,
            },
            "content": [],
            "children": [],
        });
        let html = render_blocks_html(&doc(vec![image]));
        assert!(
            html.contains(
                "<img src=\"https://example.com/cat.png\" alt=\"cat.png\" width=\"400\"/>"
            ),
            "{html}"
        );
        assert!(html.contains("<figcaption>A cat</figcaption>"), "{html}");
    }

    #[test]
    fn a_file_block_always_renders_as_a_plain_link() {
        let file = json!({
            "type": "file",
            "props": {"backgroundColor": "default", "name": "notes.pdf", "url": "https://example.com/notes.pdf", "caption": ""},
            "content": [],
            "children": [],
        });
        let html = render_blocks_html(&doc(vec![file]));
        assert!(
            html.contains("<a href=\"https://example.com/notes.pdf\">notes.pdf</a>"),
            "{html}"
        );
    }

    #[test]
    fn renders_a_two_column_layout_with_each_columns_own_content() {
        let column_list = json!({
            "type": "columnList",
            "props": {},
            "content": [],
            "children": [
                {
                    "type": "column",
                    "props": {"width": 1},
                    "content": [],
                    "children": [paragraph("Left")],
                },
                {
                    "type": "column",
                    "props": {"width": 2},
                    "content": [],
                    "children": [paragraph("Right")],
                },
            ],
        });
        let html = render_blocks_html(&doc(vec![column_list]));
        assert!(html.contains("<div class=\"wiki-columns\">"), "{html}");
        assert!(
            html.contains("<div class=\"wiki-column\" style=\"flex:1\">"),
            "{html}"
        );
        assert!(
            html.contains("<div class=\"wiki-column\" style=\"flex:2\">"),
            "{html}"
        );
        let left_pos = html.find("Left").unwrap();
        let right_pos = html.find("Right").unwrap();
        assert!(left_pos < right_pos, "{html}");
    }

    #[test]
    fn a_columns_invalid_width_falls_back_to_an_even_share() {
        let column_list = json!({
            "type": "columnList",
            "props": {},
            "content": [],
            "children": [{
                "type": "column",
                "props": {"width": -5},
                "content": [],
                "children": [paragraph("Odd")],
            }],
        });
        let html = render_blocks_html(&doc(vec![column_list]));
        assert!(
            html.contains("<div class=\"wiki-column\" style=\"flex:1\">"),
            "{html}"
        );
    }

    #[test]
    fn empty_or_malformed_content_json_renders_as_nothing() {
        assert_eq!(render_blocks_html("{}"), "");
        assert_eq!(render_blocks_html(""), "");
        assert_eq!(render_blocks_html("not json"), "");
    }

    #[test]
    fn a_page_with_no_footnotes_gets_no_footnotes_section() {
        let html = render_blocks_html(&doc(vec![paragraph("plain text, no notes")]));
        let html = with_footnotes(html);
        assert!(!html.contains("wiki-footnotes"), "{html}");
    }

    #[test]
    fn a_footnote_renders_a_numbered_marker_and_a_matching_list_entry() {
        let html = render_blocks_html(&doc(vec![json!({
            "type": "paragraph",
            "props": default_props(),
            "content": [text_item("see"), footnote_ref("the actual note text")],
            "children": [],
        })]));
        let html = with_footnotes(html);
        assert!(
            html.contains("<sup class=\"wiki-footnote-ref\" id=\"fnref-1\" data-footnote-text=\"the actual note text\"><a href=\"#fn-1\">1</a></sup>"),
            "{html}"
        );
        assert!(
            html.contains("<section class=\"wiki-footnotes\">"),
            "{html}"
        );
        assert!(
            html.contains("<li id=\"fn-1\">the actual note text <a href=\"#fnref-1\" class=\"wiki-footnote-backlink\""),
            "{html}"
        );
    }

    #[test]
    fn footnotes_number_in_document_order_across_separate_blocks() {
        let html = render_blocks_html(&doc(vec![
            json!({
                "type": "paragraph",
                "props": default_props(),
                "content": [footnote_ref("first")],
                "children": [],
            }),
            heading(2, "A heading in between"),
            json!({
                "type": "paragraph",
                "props": default_props(),
                "content": [footnote_ref("second")],
                "children": [],
            }),
        ]));
        let html = with_footnotes(html);
        let first_marker = html.find("id=\"fnref-1\"").unwrap();
        let second_marker = html.find("id=\"fnref-2\"").unwrap();
        assert!(first_marker < second_marker, "{html}");
        let list_first = html.find("id=\"fn-1\">first").unwrap();
        let list_second = html.find("id=\"fn-2\">second").unwrap();
        assert!(list_first < list_second, "{html}");
    }

    #[test]
    fn footnote_text_with_a_quote_is_attribute_escaped_in_the_inline_marker() {
        let html = render_blocks_html(&doc(vec![json!({
            "type": "paragraph",
            "props": default_props(),
            "content": [footnote_ref("a \"quoted\" note")],
            "children": [],
        })]));
        assert!(
            html.contains("data-footnote-text=\"a &quot;quoted&quot; note\""),
            "{html}"
        );
    }

    #[test]
    fn an_unsafe_color_value_is_dropped_rather_than_injected_into_the_style_attribute() {
        let html = render_blocks_html(&doc(vec![json!({
            "type": "paragraph",
            "props": {"textColor": "red\"><script>evil()</script>", "textAlignment": "left", "backgroundColor": "default"},
            "content": [text_item("hi")],
            "children": [],
        })]));
        assert!(!html.contains("<script>"), "{html}");
        assert!(
            !html.contains("style="),
            "an untrusted color value should be dropped, not rendered: {html}"
        );
    }

    #[test]
    fn link_href_with_a_quote_is_attribute_escaped() {
        let link = json!({
            "type": "link",
            "href": "https://example.com/\"onmouseover=\"evil()",
            "content": [text_item("click")],
        });
        let html = render_blocks_html(&doc(vec![json!({
            "type": "paragraph",
            "props": default_props(),
            "content": [link],
            "children": [],
        })]));
        assert!(!html.contains("onmouseover=\"evil"), "{html}");
        assert!(html.contains("&quot;"), "{html}");
    }

    proptest::proptest! {
        /// The whole point of `escape_html`: no matter what a page
        /// author types, the rendered output must never contain a raw
        /// `<` or `>` that could start or close an HTML tag.
        #[test]
        fn escape_html_never_leaves_a_raw_angle_bracket(s in ".*") {
            let escaped = escape_html(&s);
            proptest::prop_assert!(!escaped.contains('<'));
            proptest::prop_assert!(!escaped.contains('>'));
        }

        /// Every `&` the function itself emits is the start of one of
        /// its three recognized entities — never a bare `&` a browser
        /// might interpret as the start of some other, unintended
        /// entity.
        #[test]
        fn escape_html_every_ampersand_starts_a_recognized_entity(s in ".*") {
            let escaped = escape_html(&s);
            let mut search_from = 0;
            while let Some(rel_idx) = escaped[search_from..].find('&') {
                let idx = search_from + rel_idx;
                let after = &escaped[idx + 1..];
                proptest::prop_assert!(
                    after.starts_with("amp;") || after.starts_with("lt;") || after.starts_with("gt;"),
                    "unescaped & in {escaped:?} at byte {idx}"
                );
                search_from = idx + 1;
            }
        }

        /// `escape_attr` is `escape_html` plus quote-escaping — it must
        /// hold the same no-raw-angle-bracket guarantee, and must
        /// additionally never leave a raw `"` that could close an
        /// attribute value early.
        #[test]
        fn escape_attr_never_leaves_a_raw_angle_bracket_or_quote(s in ".*") {
            let escaped = escape_attr(&s);
            proptest::prop_assert!(!escaped.contains('<'));
            proptest::prop_assert!(!escaped.contains('>'));
            proptest::prop_assert!(!escaped.contains('"'));
        }

        /// Every `&` `escape_attr` emits starts one of its four
        /// recognized entities (the three from `escape_html` plus
        /// `&quot;`).
        #[test]
        fn escape_attr_every_ampersand_starts_a_recognized_entity(s in ".*") {
            let escaped = escape_attr(&s);
            let mut search_from = 0;
            while let Some(rel_idx) = escaped[search_from..].find('&') {
                let idx = search_from + rel_idx;
                let after = &escaped[idx + 1..];
                proptest::prop_assert!(
                    after.starts_with("amp;")
                        || after.starts_with("lt;")
                        || after.starts_with("gt;")
                        || after.starts_with("quot;"),
                    "unescaped & in {escaped:?} at byte {idx}"
                );
                search_from = idx + 1;
            }
        }

        /// `is_hex_color` gates whether an untrusted color string ever
        /// reaches a raw `style` attribute — it must accept exactly the
        /// shapes a CSS hex color can take (`#` + 3/4/6/8 hex digits)
        /// and reject everything else, checked against both
        /// deliberately-valid and deliberately-near-miss inputs.
        #[test]
        fn is_hex_color_accepts_exactly_well_formed_hex_colors(
            digit_count in proptest::sample::select(vec![3usize, 4, 6, 8]),
            hex_digits in "[0-9a-fA-F]{8}",
        ) {
            let value = format!("#{}", &hex_digits[..digit_count]);
            proptest::prop_assert!(is_hex_color(&value), "{value}");
        }

        /// Valid hex content without the leading `#` must be rejected
        /// — the `#` isn't optional.
        #[test]
        fn is_hex_color_rejects_a_missing_hash_prefix(hex_digits in "[0-9a-fA-F]{6}") {
            proptest::prop_assert!(!is_hex_color(&hex_digits));
        }

        /// Any digit count other than the four CSS allows (3/4/6/8)
        /// must be rejected, even with otherwise-valid hex content.
        #[test]
        fn is_hex_color_rejects_a_disallowed_digit_count(
            digit_count in proptest::sample::select(vec![0usize, 1, 2, 5, 7, 9, 10, 12]),
            hex_digits in "[0-9a-fA-F]{12}",
        ) {
            let value = format!("#{}", &hex_digits[..digit_count]);
            proptest::prop_assert!(!is_hex_color(&value), "{value}");
        }

        /// A single non-hex character anywhere in an otherwise
        /// valid-length body must be enough to reject the whole value.
        #[test]
        fn is_hex_color_rejects_a_single_non_hex_character(
            position in 0usize..6,
            hex_digits in "[0-9a-fA-F]{6}",
        ) {
            let mut body: Vec<char> = hex_digits.chars().collect();
            body[position] = 'g'; // not a hex digit
            let value = format!("#{}", body.into_iter().collect::<String>());
            proptest::prop_assert!(!is_hex_color(&value), "{value}");
        }

        /// `slugify`'s own three guarantees: ASCII lowercase
        /// alphanumerics and single-hyphen separators only, never a
        /// leading/trailing/doubled hyphen, and never empty (an
        /// all-symbol title falls back to a fixed default rather than
        /// an empty slug).
        #[test]
        fn slugify_produces_a_safe_nonempty_slug(title in ".*") {
            let slug = slugify(&title);
            proptest::prop_assert!(!slug.is_empty());
            proptest::prop_assert!(
                slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            );
            proptest::prop_assert!(!slug.starts_with('-'));
            proptest::prop_assert!(!slug.ends_with('-'));
            proptest::prop_assert!(!slug.contains("--"));
        }

        /// Every step `strip_tags` performs — dropping characters
        /// between `<`/`>`, or replacing a multi-character entity with
        /// a single literal character — only ever removes content,
        /// never adds any: the output can never be longer than the
        /// input.
        #[test]
        fn strip_tags_never_grows_the_input(s in ".*") {
            let stripped = strip_tags(&s);
            proptest::prop_assert!(stripped.len() <= s.len());
        }

        /// The entire reason `unique_heading_id` exists: feed it any
        /// sequence of heading texts sharing the same `used` map (the
        /// same shape as `srs.rs`'s own state-evolves-through-a-
        /// sequence property test), and every id it has ever returned
        /// must be unique — duplicate ids would mean two headings'
        /// in-page anchors collide.
        #[test]
        fn unique_heading_id_never_repeats_an_id_across_a_sequence(
            texts in proptest::collection::vec(".*", 0..20),
        ) {
            let mut used = std::collections::HashMap::new();
            let mut seen = std::collections::HashSet::new();
            for text in texts {
                let id = unique_heading_id(&text, &mut used);
                proptest::prop_assert!(seen.insert(id.clone()), "duplicate id: {id}");
            }
        }
    }
}
