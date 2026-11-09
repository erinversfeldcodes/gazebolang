import { createRoot, type Root } from "react-dom/client";
import { useMemo, useState, useCallback, useRef, useEffect, type ReactElement, type KeyboardEvent } from "react";
import { type Block, type BlockNoteEditor, BlockNoteSchema, COLORS_DEFAULT, createBlockSpec, defaultBlockSpecs, defaultInlineContentSpecs, getColspan, getDefaultBlockTypeItems, getRowspan, isTableCell } from "@blocknote/core";
// The exact logic BlockNote's own "/" menu uses to insert each media type
// (including opening the upload panel immediately after) — reused as-is
// rather than reimplemented, so Image/Video/Audio/File/Emoji behave
// identically to picking them from "/"; only how they're *presented*
// (a flat list on the toolbar, not a nested/scrolling category menu)
// changes. Public API: @blocknote/core/extensions is a real subpath
// export (see its package.json), not an internal path.
import { getDefaultSlashMenuItems, TableHandlesExtension, type DefaultSuggestionItem } from "@blocknote/core/extensions";
import { createReactInlineContentSpec, useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import { withMultiColumn } from "@blocknote/xl-multi-column";
import "@blocknote/core/fonts/inter.css";
import "@blocknote/mantine/style.css";

// A table of contents used to be represented as a plain paragraph
// containing the literal text "[TOC]" — a magic string crates/web/src/
// wiki.rs's renderer looked for and spliced a real "Contents" box into at
// render time. While editing, that showed as ordinary, editable text with
// no indication it was special, easy to mistake for a stray typo and easy
// to mangle by accident (confirmed live: it reads as something you could
// just... edit). A real block type fixes both: content "none" means
// nothing can be typed inside it, and render() below draws an obviously-
// not-text placeholder instead. wiki.rs's render_single_block matches
// this exact block type ("tableOfContents") server-side now, in place of
// the old marker-paragraph text search.
const TableOfContentsBlock = createBlockSpec(
  {
    type: "tableOfContents" as const,
    propSchema: {},
    content: "none",
  },
  {
    render: () => {
      const dom = document.createElement("div");
      dom.className = "wiki-toc-placeholder";
      dom.contentEditable = "false";

      const label = document.createElement("span");
      label.className = "wiki-toc-placeholder-label";
      const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      icon.setAttribute("viewBox", "0 0 24 24");
      icon.setAttribute("fill", "none");
      icon.setAttribute("stroke", "currentColor");
      icon.setAttribute("stroke-width", "2");
      icon.setAttribute("stroke-linecap", "round");
      icon.setAttribute("stroke-linejoin", "round");
      for (const [x1, y1, x2, y2] of [
        ["4", "6", "20", "6"],
        ["4", "12", "15", "12"],
        ["4", "18", "17", "18"],
      ]) {
        const line = document.createElementNS("http://www.w3.org/2000/svg", "line");
        line.setAttribute("x1", x1);
        line.setAttribute("y1", y1);
        line.setAttribute("x2", x2);
        line.setAttribute("y2", y2);
        icon.appendChild(line);
      }
      label.appendChild(icon);
      label.appendChild(document.createTextNode("Table of contents"));
      dom.appendChild(label);

      const hint = document.createElement("small");
      hint.textContent = "Generated automatically from this page's headings when saved";
      dom.appendChild(hint);

      return { dom };
    },
  },
)();

// Walks `blocks` (the document, or any block's `children`) collecting every
// footnoteRef's `noteId`, document-order depth-first — table cells included,
// since a table cell's `content` is plain inline content same as a
// paragraph's, just reached through `content.rows[].cells[]` instead of
// directly. This is purely a client-side numbering aid: `noteId` itself
// never has to mean anything to crates/web/src/wiki.rs's read-view
// renderer, which assigns its own numbers the same way — just counting
// occurrences as it walks the saved content_json, server-side.
function collectFootnoteIds(blocks: Block<any, any, any>[], ids: string[]) {
  for (const block of blocks) {
    const content = block.content as unknown;
    if (Array.isArray(content)) {
      for (const item of content as { type?: string; props?: { noteId?: string } }[]) {
        if (item.type === "footnoteRef" && item.props?.noteId) {
          ids.push(item.props.noteId);
        }
      }
    } else if (content && typeof content === "object" && "rows" in content) {
      for (const row of (content as { rows: { cells: unknown[] }[] }).rows) {
        for (const cell of row.cells as { content?: unknown }[]) {
          if (Array.isArray(cell.content)) {
            for (const item of cell.content as { type?: string; props?: { noteId?: string } }[]) {
              if (item.type === "footnoteRef" && item.props?.noteId) {
                ids.push(item.props.noteId);
              }
            }
          }
        }
      }
    }
    if (block.children?.length) {
      collectFootnoteIds(block.children, ids);
    }
  }
}

// Falls back to "one past the end" rather than 0 for a noteId not found yet
// (the brief moment between insertCFootnote's editor.insertInlineContent
// call and this component's own first render of the freshly-inserted node)
// — a transient, too-high number beats a transient "0th footnote", neither
// being what the saved document will actually show once this settles.
function footnoteNumber(document: Block<any, any, any>[], noteId: string): number {
  const ids: string[] = [];
  collectFootnoteIds(document, ids);
  const index = ids.indexOf(noteId);
  return index === -1 ? ids.length + 1 : index + 1;
}

// A plain-text annotation, not rich content — content: "none" (an atomic
// node storing its text as a prop, like a mention) rather than "styled"
// (nested editable inline content): BlockNote's own content model would
// let a "styled" footnote hold bold/links/etc., but there's nowhere for
// that richness to go once it reaches wiki.rs's render_inline_item, which
// reads this same text back out of a plain HTML attribute for both the
// inline marker's tooltip-free click target and the generated footnotes
// list — matching what's actually renderable server-side rather than
// promising formatting that would silently flatten to plain text on save.
const FootnoteRef = createReactInlineContentSpec(
  {
    type: "footnoteRef",
    propSchema: {
      // Assigned once, at insert time (see the toolbar's insertFootnote
      // below) — this component's own identity for footnoteNumber above,
      // stable across reorders/edits elsewhere in the document. Never
      // read by wiki.rs: the read view numbers footnotes purely by the
      // order it encounters them while walking the saved content, same
      // as this component does for its own live preview, just without
      // needing a stable id of its own (nothing there persists across a
      // re-render the way a mounted React node does).
      noteId: { default: "" },
      text: { default: "" },
    },
    content: "none",
  },
  {
    render: (props) => <FootnoteRefView {...props} />,
  },
);

// Narrowly typed to just what this component actually reads/calls, rather
// than fighting ReactCustomInlineContentRenderProps's own generics down to
// the literal "footnoteRef" config — `any` for `editor`, matching how the
// rest of this file already routes around BlockNote's heavily-generic
// types (see stripBlockIds above).
function FootnoteRefView({
  inlineContent,
  updateInlineContent,
  editor,
  contentRef,
}: {
  inlineContent: { props: { noteId: string; text: string } };
  updateInlineContent: (update: {
    type: "footnoteRef";
    props: { noteId: string; text: string };
  }) => void;
  editor: BlockNoteEditor<any, any, any>;
  contentRef: (node: HTMLElement | null) => void;
}) {
  // Opens immediately for a just-inserted (still-empty) footnote, so
  // inserting one and typing its text is a single motion — not for an
  // existing one being merely clicked past/through while editing
  // elsewhere on the page.
  const [open, setOpen] = useState(inlineContent.props.text === "");
  const [text, setText] = useState(inlineContent.props.text);
  const [number, setNumber] = useState(() => footnoteNumber(editor.document, inlineContent.props.noteId));
  // Wraps the marker *and* the popover together — not just the popover —
  // for the same reason the toolbar's own click-away popovers (e.g.
  // wiki-toolbar-table's tableRef) wrap their trigger button too: confirmed
  // live that scoping this ref to the popover alone closes the popover the
  // instant it opens, every time, on a real click (though not import a
  // synthetic one dispatched directly via JS) — the click that opens it
  // bubbles to this effect's own freshly-attached document listener before
  // finishing propagation, and since that click's target (the button)
  // wasn't "inside" a ref scoped to the popover alone, it read as an
  // outside click closing what it had just opened.
  const containerRef = useRef<HTMLElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => setText(inlineContent.props.text), [inlineContent.props.text]);

  useEffect(() => {
    const recompute = () => setNumber(footnoteNumber(editor.document, inlineContent.props.noteId));
    recompute();
    return editor.onChange(recompute);
  }, [editor, inlineContent.props.noteId]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (ev: MouseEvent) => {
      if (!containerRef.current?.contains(ev.target as Node)) setOpen(false);
    };
    // Both the focus call and the listener registration below are
    // deferred to the next macrotask, not run inline on mount/open.
    // Confirmed live, two separate symptoms, one cause: run synchronously,
    // textareaRef.current.focus() lost a race against BlockNote's own
    // post-transaction editor.focus() (every insertFootnote-style
    // transaction refocuses the ProseMirror view right after running,
    // stealing it back from a plain `autoFocus` or an inline .focus()
    // call), and the mousedown listener lost a race against the very
    // click that caused `open` to become true in the first place — still
    // bubbling at the moment a synchronously-attached listener would have
    // fired (containerRef above only exempts a click on this footnote's
    // own marker from that; it does nothing for insertFootnote's
    // auto-open, whose triggering click is the *toolbar's* button,
    // legitimately outside this footnote's own container). Both finish
    // before this timer's callback runs, so deferring past them — rather
    // than trying to win by running earlier — is what actually works.
    const timer = setTimeout(() => {
      textareaRef.current?.focus();
      document.addEventListener("mousedown", onPointerDown);
    }, 0);
    return () => {
      clearTimeout(timer);
      document.removeEventListener("mousedown", onPointerDown);
    };
  }, [open]);

  const commit = () => {
    if (text !== inlineContent.props.text) {
      updateInlineContent({
        type: "footnoteRef",
        props: { ...inlineContent.props, text },
      });
    }
  };

  return (
    <sup
      className="wiki-footnote-ref"
      ref={(el) => {
        contentRef(el);
        containerRef.current = el;
      }}
    >
      <button
        type="button"
        className="wiki-footnote-marker"
        title={text || "Empty footnote"}
        onMouseDown={(ev) => {
          ev.preventDefault();
          setOpen((o) => !o);
        }}
      >
        {number}
      </button>
      {open && (
        <span className="wiki-footnote-popover">
          <textarea
            ref={textareaRef}
            placeholder="Footnote text"
            value={text}
            onChange={(ev) => setText(ev.target.value)}
            onBlur={commit}
            onKeyDown={(ev) => {
              if (ev.key === "Escape") {
                setText(inlineContent.props.text);
                setOpen(false);
              } else if (ev.key === "Enter" && !ev.shiftKey) {
                ev.preventDefault();
                commit();
                setOpen(false);
              }
            }}
          />
        </span>
      )}
    </sup>
  );
}

// The default schema (every block/style/inline-content type `BlockNoteSchema.create()`
// ships with no arguments — confirmed in its own source) plus `column`/
// `columnList` (side-by-side layout, from `@blocknote/xl-multi-column`),
// `tableOfContents` above, and `footnoteRef` above. Module-level and
// singular: every editor instance this island ever mounts shares the same
// schema, same as before this had a name at all.
const wikiSchema = withMultiColumn(
  BlockNoteSchema.create({
    blockSpecs: {
      ...defaultBlockSpecs,
      tableOfContents: TableOfContentsBlock,
    },
    inlineContentSpecs: {
      ...defaultInlineContentSpecs,
      footnoteRef: FootnoteRef,
    },
  }),
);

// The precise editor type `useCreateBlockNote({ schema: wikiSchema })`
// actually returns (confirmed in its own source: passing `schema` flips its
// return type via a conditional type keyed off that schema's own generics)
// — replaces the bare, default-schema `BlockNoteEditor` that doesn't know
// about `column`/`columnList`, which this toolbar's insert-columns buttons
// need.
type WikiBlockNoteEditor = BlockNoteEditor<
  typeof wikiSchema.blockSchema,
  typeof wikiSchema.inlineContentSchema,
  typeof wikiSchema.styleSchema
>;

// Used by formatSelectionAsColumns below to reuse a selected block as a
// column's child. BlockNote only generates a fresh id for an inserted
// block when none is given — passing one through unchanged would collide
// with its own still-live original for the moment both exist mid-
// transaction (the new columnList is inserted before the old blocks are
// removed). `any`, matching how the rest of this file routes around
// BlockNote's own heavily-generic block types rather than fighting them.
function stripBlockIds(block: Block<any, any, any>): any {
  return {
    type: block.type,
    props: block.props,
    content: block.content,
    children: block.children.map(stripBlockIds),
  };
}

// Used by the toolbar's "Remove columns" item: walks the document to find
// the columnList, if any, that the block with `blockId` currently lives
// inside — so that item can appear whenever the cursor is anywhere inside
// an existing layout, not only when there's an active text selection
// (formatSelectionAsColumns' own hasColumnSelection gate), since removing a
// layout you're just sitting in doesn't need anything selected.
function findEnclosingColumnList(blocks: Block<any, any, any>[], blockId: string): any | undefined {
  const contains = (block: any): boolean => block.id === blockId || (block.children ?? []).some(contains);
  for (const block of blocks) {
    if (block.type === "columnList" && block.children.some(contains)) {
      return block;
    }
    if (block.children?.length) {
      const found = findEnclosingColumnList(block.children, blockId);
      if (found) return found;
    }
  }
  return undefined;
}

// Used by formatSelectionAsColumns below when there are fewer selected
// blocks than requested columns (most commonly: one highlighted paragraph
// or heading, asked for 2 or 3 columns) — splitting whole blocks can't fill
// every column then no matter how they're sliced, so this splits the text
// itself instead. Confirmed live this was the real "columns don't split
// evenly" complaint: a single heading landed entirely in column one,
// leaving the rest empty, every time, regardless of how long that heading
// was. Only "text" inline nodes are ever cut mid-run — at the nearest
// whitespace within `window` characters of the exact target length when
// there is one nearby (so ordinary prose doesn't get split mid-word), a
// hard character cut otherwise (so one long unbroken run, e.g. a URL or a
// stress-test string with no spaces at all, still actually splits instead
// of silently refusing to). Any other inline content (links, etc.) is kept
// whole and counted as a single character for sizing, so it can still land
// on either side of a boundary without being split itself.
function splitInlineContent(content: any[], count: number): any[][] {
  const nodeLength = (node: any) => (node.type === "text" ? (node.text as string).length : 1);
  const total = content.reduce((sum, node) => sum + nodeLength(node), 0);
  const target = total / count;
  const pieces: any[][] = Array.from({ length: count }, () => []);
  let col = 0;
  let colLength = 0;
  const window = 15;
  for (const node of content) {
    if (node.type !== "text") {
      pieces[col].push(node);
      colLength += 1;
      if (col < count - 1 && colLength >= target) {
        col++;
        colLength = 0;
      }
      continue;
    }
    let text: string = node.text;
    while (col < count - 1 && colLength + text.length > target) {
      const idealCut = Math.min(Math.max(Math.round(target - colLength), 1), text.length);
      let cut = idealCut;
      for (let d = 0; d <= window; d++) {
        if (idealCut - d > 0 && /\s/.test(text[idealCut - d - 1])) {
          cut = idealCut - d;
          break;
        }
        if (idealCut + d < text.length && /\s/.test(text[idealCut + d])) {
          cut = idealCut + d;
          break;
        }
      }
      pieces[col].push({ ...node, text: text.slice(0, cut) });
      text = text.slice(cut);
      col++;
      colLength = 0;
    }
    if (text.length > 0) {
      pieces[col].push({ ...node, text });
      colLength += text.length;
    }
  }
  return pieces;
}

// Minimal stroke icons (Feather-style: 24x24 viewBox, 2px stroke,
// currentColor) for the toolbar — Confluence's own toolbar is icon-only,
// not text labels, and that's most of what made the earlier version read
// as "not a real editor toolbar" rather than any single missing feature.
function IconBold() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M6 4h8a4 4 0 0 1 4 4 4 4 0 0 1-4 4H6z" />
      <path d="M6 12h9a4 4 0 0 1 4 4 4 4 0 0 1-4 4H6z" />
    </svg>
  );
}

function IconItalic() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="19" y1="4" x2="10" y2="4" />
      <line x1="14" y1="20" x2="5" y2="20" />
      <line x1="15" y1="4" x2="9" y2="20" />
    </svg>
  );
}

function IconUnderline() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M6 3v7a6 6 0 0 0 6 6 6 6 0 0 0 6-6V3" />
      <line x1="4" y1="21" x2="20" y2="21" />
    </svg>
  );
}

function IconStrikethrough() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="4" y1="12" x2="20" y2="12" />
      <path d="M16 6.5C15.5 5 14 4 11.5 4S7 5.2 7 7c0 1.6 1.2 2.6 3 3.2" />
      <path d="M8 17.5c.5 1.5 2 2.5 4.5 2.5S17 18.8 17 17c0-1.6-1.2-2.6-3-3.2" />
    </svg>
  );
}

function IconLink() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
      <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
    </svg>
  );
}

function IconTable() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="4" width="18" height="16" rx="1.5" />
      <line x1="3" y1="10" x2="21" y2="10" />
      <line x1="3" y1="16" x2="21" y2="16" />
      <line x1="9" y1="4" x2="9" y2="20" />
      <line x1="15" y1="4" x2="15" y2="20" />
    </svg>
  );
}

function IconMergeCells() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="4" width="18" height="16" rx="1.5" />
      <line x1="12" y1="4" x2="12" y2="9" />
      <line x1="12" y1="15" x2="12" y2="20" />
      <polyline points="9,10 12,13 15,10" />
      <polyline points="9,14 12,11 15,14" />
    </svg>
  );
}

function IconSplitCells() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="4" width="18" height="16" rx="1.5" />
      <line x1="12" y1="4" x2="12" y2="20" />
      <polyline points="9,9 6,12 9,15" />
      <polyline points="15,9 18,12 15,15" />
    </svg>
  );
}

function IconPlus() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="12" y1="5" x2="12" y2="19" />
      <line x1="5" y1="12" x2="19" y2="12" />
    </svg>
  );
}

function IconImage() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="3" width="18" height="18" rx="2" />
      <circle cx="8.5" cy="8.5" r="1.5" />
      <path d="M21 15l-5-5L5 21" />
    </svg>
  );
}

function IconVideo() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="2" y="5" width="15" height="14" rx="2" />
      <path d="M17 10l5-3v10l-5-3z" />
    </svg>
  );
}

function IconAudio() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polygon points="5 7 5 17 10 17 15 21 15 3 10 7 5 7" />
      <path d="M18 9a4 4 0 0 1 0 6" />
    </svg>
  );
}

function IconFile() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M6 2h9l5 5v15H6z" />
      <path d="M15 2v5h5" />
    </svg>
  );
}

function IconEmoji() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="9" />
      <path d="M8.5 14.5c1 1.2 2.1 1.8 3.5 1.8s2.5-.6 3.5-1.8" />
      <line x1="9" y1="9.5" x2="9.01" y2="9.5" />
      <line x1="15" y1="9.5" x2="15.01" y2="9.5" />
    </svg>
  );
}

function IconQuote() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M7 15c-1.5 0-2.5-1-2.5-2.5S5.5 10 7 10c0-2 1-3.5 3-4" />
      <path d="M16 15c-1.5 0-2.5-1-2.5-2.5s1-2.5 2.5-2.5c0-2 1-3.5 3-4" />
    </svg>
  );
}

function IconBulletList() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="4.5" cy="6" r="1.3" fill="currentColor" stroke="none" />
      <circle cx="4.5" cy="12" r="1.3" fill="currentColor" stroke="none" />
      <circle cx="4.5" cy="18" r="1.3" fill="currentColor" stroke="none" />
      <line x1="9.5" y1="6" x2="20.5" y2="6" />
      <line x1="9.5" y1="12" x2="20.5" y2="12" />
      <line x1="9.5" y1="18" x2="20.5" y2="18" />
    </svg>
  );
}

function IconNumberedList() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
      <text x="2" y="8.5" fontSize="7" fill="currentColor" stroke="none" fontFamily="sans-serif">1</text>
      <text x="2" y="14.5" fontSize="7" fill="currentColor" stroke="none" fontFamily="sans-serif">2</text>
      <text x="2" y="20.5" fontSize="7" fill="currentColor" stroke="none" fontFamily="sans-serif">3</text>
      <line x1="9.5" y1="6" x2="20.5" y2="6" />
      <line x1="9.5" y1="12" x2="20.5" y2="12" />
      <line x1="9.5" y1="18" x2="20.5" y2="18" />
    </svg>
  );
}

function IconCheckList() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="4" width="5.5" height="5.5" rx="1.2" />
      <path d="M4.4 6.8l1 1 2-2.3" />
      <rect x="3" y="15" width="5.5" height="5.5" rx="1.2" />
      <line x1="11.5" y1="6.5" x2="21" y2="6.5" />
      <line x1="11.5" y1="17.5" x2="21" y2="17.5" />
    </svg>
  );
}

function IconToggleList() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M5.5 8.5l3 3.5-3 3.5" />
      <line x1="11.5" y1="7" x2="21" y2="7" />
      <line x1="11.5" y1="12" x2="21" y2="12" />
      <line x1="11.5" y1="17" x2="21" y2="17" />
    </svg>
  );
}

function IconCodeBlock() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="9 7 4 12 9 17" />
      <polyline points="15 7 20 12 15 17" />
    </svg>
  );
}

function IconDivider() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="4" y1="12" x2="20" y2="12" />
      <line x1="4" y1="9" x2="4" y2="15" />
      <line x1="20" y1="9" x2="20" y2="15" />
    </svg>
  );
}

function IconTableOfContents() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="4" y1="6" x2="20" y2="6" />
      <line x1="4" y1="12" x2="15" y2="12" />
      <line x1="4" y1="18" x2="17" y2="18" />
    </svg>
  );
}

function IconFootnote() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="4" y1="7" x2="16" y2="7" />
      <line x1="4" y1="13" x2="12" y2="13" />
      <path d="M17 15 L21 19 M21 15 L17 19" />
    </svg>
  );
}

function IconInlineCode() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="8 8 4 12 8 16" />
      <polyline points="16 8 20 12 16 16" />
    </svg>
  );
}

function IconClearFormatting() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <text x="2" y="16" fontSize="14" fill="currentColor" stroke="none" fontFamily="serif">A</text>
      <line x1="3" y1="20" x2="21" y2="5" />
    </svg>
  );
}

function IconTextColor() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <text x="3" y="16" fontSize="14" fill="currentColor" stroke="none" fontFamily="serif">A</text>
      <rect x="2" y="19" width="20" height="3" fill="currentColor" stroke="none" />
    </svg>
  );
}

function IconHighlight() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M9 11l6-6 4 4-6 6" />
      <path d="M8 12l4 4-2.5 2.5H5V14.5z" fill="currentColor" stroke="none" />
      <line x1="3" y1="21" x2="21" y2="21" />
    </svg>
  );
}

function IconAlignLeft() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="4" y1="6" x2="20" y2="6" />
      <line x1="4" y1="12" x2="14" y2="12" />
      <line x1="4" y1="18" x2="17" y2="18" />
    </svg>
  );
}

function IconAlignCenter() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="4" y1="6" x2="20" y2="6" />
      <line x1="7" y1="12" x2="17" y2="12" />
      <line x1="5.5" y1="18" x2="18.5" y2="18" />
    </svg>
  );
}

function IconAlignRight() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="4" y1="6" x2="20" y2="6" />
      <line x1="10" y1="12" x2="20" y2="12" />
      <line x1="7" y1="18" x2="20" y2="18" />
    </svg>
  );
}

function IconAlignJustify() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="4" y1="6" x2="20" y2="6" />
      <line x1="4" y1="12" x2="20" y2="12" />
      <line x1="4" y1="18" x2="20" y2="18" />
    </svg>
  );
}

function IconColumns2() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="4" width="8" height="16" rx="1" />
      <rect x="13" y="4" width="8" height="16" rx="1" />
    </svg>
  );
}

function IconColumns3() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="2.5" y="4" width="5.5" height="16" rx="1" />
      <rect x="9.3" y="4" width="5.5" height="16" rx="1" />
      <rect x="16" y="4" width="5.5" height="16" rx="1" />
    </svg>
  );
}

function IconSingleColumn() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="4" width="18" height="16" rx="1" />
    </svg>
  );
}

// Keyed by the item's `key` from getDefaultSlashMenuItems — only the
// handful of "long tail" items the toolbar doesn't otherwise materialize
// as a dedicated control (block-type swaps live in the <select>; Table has
// its own button with a grid picker) end up behind the Insert button.
const INSERT_ITEM_ICONS: Record<string, () => ReactElement> = {
  image: IconImage,
  video: IconVideo,
  audio: IconAudio,
  file: IconFile,
  emoji: IconEmoji,
  divider: IconDivider,
};
// Confluence's own "+" insert menu (the reference this toolbar is built
// against) puts Divider here too, not on the main toolbar — it's an
// occasional, position-specific insert ("add a dividing line here"), not
// a block type you'd ever retype an existing paragraph into, so it keeps
// BlockNote's own insert-a-new-block semantics (getDefaultSlashMenuItems'
// onItemClick) rather than the toolbar <select>/BASIC_BLOCK_BUTTONS'
// retype-the-current-block one.
const INSERT_ITEM_KEYS = ["image", "video", "audio", "file", "emoji", "divider"];

// A real, always-on toolbar rather than BlockNote's default formatting
// controls (bold/italic/underline/strike, by far the most-reached-for
// styling, each independent of the others — select a run of text, click,
// done), block type (paragraph/heading levels/lists) via a dropdown, same
// as every word processor's style picker. No font-family control: that's
// not a concept BlockNote's block model has (block-based editors style by
// semantic type, not arbitrary per-run font swapping the way a classic
// word processor does) — flagged honestly rather than faked.
//
// onMouseDown + preventDefault, not onClick, on every button: a click
// first fires mousedown, which would move focus to the button and collapse
// the editor's text selection *before* the click handler ever runs,
// so the style would end up applying to nothing. Preventing default on
// mousedown keeps focus (and the selection) in the editor throughout.
const MARK_BUTTONS: {
  icon: () => ReactElement;
  title: string;
  style: "bold" | "italic" | "underline" | "strike" | "code";
}[] = [
  { icon: IconBold, title: "Bold", style: "bold" },
  { icon: IconItalic, title: "Italic", style: "italic" },
  { icon: IconUnderline, title: "Underline", style: "underline" },
  { icon: IconStrikethrough, title: "Strikethrough", style: "strike" },
  // Inline code — a short code/variable name within a sentence, distinct
  // from the Code Block button in BASIC_BLOCK_BUTTONS (a whole separate
  // block). Both exist as independent marks/blocks in BlockNote's default
  // schema already; this was just never wired to a button.
  { icon: IconInlineCode, title: "Code", style: "code" },
];

// Every "Basic blocks" item except Paragraph gets a toolbar button, no
// exceptions — confirmed live that Confluence's own "Normal text" dropdown
// actually keeps Quote alongside its heading levels (not a toolbar
// button), but the explicit ask here was that *no* basic block should be
// left sitting in the style menu, so Quote joins the others rather than
// becoming a one-off carve-out. Divider is the only remaining exception,
// in the "+" insert menu instead (see INSERT_ITEM_KEYS above) — it's not a
// block you'd ever retype existing text into, only ever insert fresh.
// Uses `applyBlockType` (below) with these items' own dictionary keys so
// they stay in sync with the <select>'s lookup table rather than
// duplicating type/props by hand.
const BASIC_BLOCK_BUTTONS: { icon: () => ReactElement; title: string; key: string }[] = [
  { icon: IconQuote, title: "Quote", key: "quote" },
  { icon: IconBulletList, title: "Bullet list", key: "bullet_list" },
  { icon: IconNumberedList, title: "Numbered list", key: "numbered_list" },
  { icon: IconCheckList, title: "Checklist", key: "check_list" },
  { icon: IconToggleList, title: "Toggle list", key: "toggle_list" },
  { icon: IconCodeBlock, title: "Code block", key: "code_block" },
];

// Standard across Word/Docs/Notion/Confluence: an 8x8 cap on the picker
// grid itself, not on what you can insert — anything bigger is a rare
// enough case that typing "/table" (or just resizing after inserting) is
// fine, and 8x8 is the size every one of those editors ships by default.
const TABLE_PICKER_SIZE = 8;

// Only the two groups that actually belong in a paragraph-style dropdown —
// BASIC_BLOCK_BUTTONS above now covers everything BlockNote's own
// dictionary puts in "Basic blocks" except paragraph itself, which the
// <select> still offers as a bare top-level option (same convention as a
// word processor's Styles dropdown including "Normal text" alongside its
// heading levels).
const SELECT_GROUP_ORDER = ["Headings", "Subheadings"];

const ALIGN_BUTTONS: { icon: () => ReactElement; title: string; alignment: "left" | "center" | "right" | "justify" }[] = [
  { icon: IconAlignLeft, title: "Align left", alignment: "left" },
  { icon: IconAlignCenter, title: "Align center", alignment: "center" },
  { icon: IconAlignRight, title: "Align right", alignment: "right" },
  { icon: IconAlignJustify, title: "Justify", alignment: "justify" },
];

// BlockNote's own 9-swatch Notion-style palette (`COLORS_DEFAULT`, from
// `@blocknote/core`) — the only values `wiki.rs`'s `render_blocks_html`
// resolves to an actual CSS color on the read view (anything else is
// dropped, not guessed at), so these are the only swatches offered here
// too, rather than a freeform color input nothing downstream would honor.
const COLOR_NAMES = Object.keys(COLORS_DEFAULT);

function Toolbar({ editor }: { editor: WikiBlockNoteEditor }) {
  const [linkOpen, setLinkOpen] = useState(false);
  const [linkUrl, setLinkUrl] = useState("");
  // Captured once, when the popover opens — createLink(url) with no
  // second argument doesn't keep the selected text as the link's label,
  // it just replaces the selection with an empty link (confirmed live:
  // the words vanished entirely). Passing the text back explicitly is
  // what actually preserves it.
  const [linkText, setLinkText] = useState("");
  const [insertOpen, setInsertOpen] = useState(false);
  const [tableOpen, setTableOpen] = useState(false);
  const [tableHover, setTableHover] = useState<{ row: number; col: number } | null>(null);
  const [textColorOpen, setTextColorOpen] = useState(false);
  const [highlightOpen, setHighlightOpen] = useState(false);
  const [columnsOpen, setColumnsOpen] = useState(false);
  const insertRef = useRef<HTMLSpanElement>(null);
  const linkRef = useRef<HTMLSpanElement>(null);
  const tableRef = useRef<HTMLSpanElement>(null);
  const textColorRef = useRef<HTMLSpanElement>(null);
  const highlightRef = useRef<HTMLSpanElement>(null);
  const columnsRef = useRef<HTMLSpanElement>(null);

  // Click-away dismissal for every popover — each already closes itself on
  // its own "do the thing" action (Apply, picking an insert item, clicking
  // a grid cell or a swatch), but clicking anywhere else (the editor body,
  // another toolbar button) left it stuck open with nothing to close it,
  // which is its own "nothing works like a real editor" paper cut. One
  // listener for all of them rather than one near-identical effect each.
  useEffect(() => {
    if (!insertOpen && !linkOpen && !tableOpen && !textColorOpen && !highlightOpen && !columnsOpen) return;
    const onPointerDown = (ev: MouseEvent) => {
      const target = ev.target as Node;
      if (insertOpen && !insertRef.current?.contains(target)) setInsertOpen(false);
      if (linkOpen && !linkRef.current?.contains(target)) setLinkOpen(false);
      if (tableOpen && !tableRef.current?.contains(target)) {
        setTableOpen(false);
        setTableHover(null);
      }
      if (textColorOpen && !textColorRef.current?.contains(target)) setTextColorOpen(false);
      if (highlightOpen && !highlightRef.current?.contains(target)) setHighlightOpen(false);
      if (columnsOpen && !columnsRef.current?.contains(target)) setColumnsOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [insertOpen, linkOpen, tableOpen, textColorOpen, highlightOpen, columnsOpen]);

  // Columns are something you apply to an existing selection (see
  // formatSelectionAsColumns below), not something you insert empty and
  // fill in afterward — so the "2/3 columns" items need to know whether a
  // real (non-collapsed) selection is currently active, to grey themselves
  // out otherwise rather than silently doing nothing when clicked.
  // getSelection() returns undefined for a plain collapsed cursor but a
  // one-block array for ordinary highlighted text within a single
  // paragraph (confirmed against its own source — only a fully collapsed
  // selection or a selected non-text node returns undefined), so a single
  // selected block is enough: it becomes column one, with an empty column
  // two to type into. onSelectionChange is BlockNote's own event for this;
  // getSelection() alone isn't reactive, so it can't just be read during
  // render.
  const [hasColumnSelection, setHasColumnSelection] = useState(false);
  // The columnList, if any, the cursor is currently inside — drives the
  // toolbar's "Remove columns" item (see findEnclosingColumnList above and
  // removeColumns below). Tracked in the same onSelectionChange subscription
  // as hasColumnSelection rather than a second one: both only need to
  // recompute when the cursor/selection actually moves.
  const [enclosingColumnList, setEnclosingColumnList] = useState<any | null>(null);
  // Mirrors what BlockNote's own (unused here — see formattingToolbar={false}
  // below) TableCellMergeButton/SplitButton compute internally, just read
  // directly off the editor instead of through their context-bound hooks,
  // since this toolbar is a sibling of <BlockNoteView>, not a child of it,
  // and so sits outside BlockNoteContext. getCellSelection() resolves a cell
  // from a plain cursor position, not just a real multi-cell drag-selection
  // (confirmed in its own source), so "canSplit" reflects whichever single
  // cell the cursor is currently in, and "mergeDirection" stays undefined
  // until that selection actually spans more than one cell.
  const [tableCellAction, setTableCellAction] = useState<{
    mergeDirection: "horizontal" | "vertical" | undefined;
    canSplit: boolean;
  } | null>(null);
  // Drives the pressed look on MARK_BUTTONS below — confirmed live this
  // toolbar had no active-state feedback at all for Bold/Italic/etc.,
  // unlike the block-type <select> (which already reflects the current
  // block) or BlockNote's own default formatting toolbar (unused here,
  // see formattingToolbar={false} below, but it does this too). Keyed by
  // BlockNote's own style name ("bold", "italic", ...), same names
  // MARK_BUTTONS already uses — getActiveStyles() reads the cursor's
  // stored marks when the selection is collapsed (so toggling Bold with
  // nothing selected still shows pressed immediately, before any text
  // exists to carry the mark) and the selection's own marks otherwise.
  const [activeStyles, setActiveStyles] = useState<Record<string, boolean | string>>({});
  useEffect(
    () =>
      editor.onSelectionChange(() => {
        setHasColumnSelection((editor.getSelection()?.blocks.length ?? 0) >= 1);
        const { block } = editor.getTextCursorPosition();
        setEnclosingColumnList(findEnclosingColumnList(editor.document, block.id) ?? null);
        setActiveStyles(editor.getActiveStyles());

        const selectedBlocks = editor.getSelection()?.blocks ?? [block];
        const tableHandles = editor.getExtension(TableHandlesExtension);
        if (selectedBlocks.length !== 1 || selectedBlocks[0].type !== "table" || !tableHandles) {
          setTableCellAction(null);
          return;
        }
        const tableBlock = selectedBlocks[0] as Parameters<typeof tableHandles.getMergeDirection>[0];
        const cellSelection = tableHandles.getCellSelection();
        const cell = cellSelection && tableBlock?.content.rows[cellSelection.from.row]?.cells[cellSelection.from.col];
        setTableCellAction({
          mergeDirection: tableHandles.getMergeDirection(tableBlock),
          canSplit: Boolean(cell && isTableCell(cell) && (getRowspan(cell) > 1 || getColspan(cell) > 1)),
        });
      }),
    [editor],
  );

  // getDefaultBlockTypeItems is BlockNote's own purpose-built source for
  // exactly this control (its doc comment names "the slash menu and the
  // block type select" as its two consumers) — paragraph, every heading
  // level the schema allows (1-6, confirmed live via the default schema's
  // `levels` option), and the three toggleable headings, plus quote and
  // all four list types, each already carrying the right `type`/`props`
  // pair (a heading's `level` and, for a toggle heading, `isToggleable`).
  // Reusing it directly means this list — and in particular headings 4-6
  // and the toggle headings — can't silently drift out of sync with what
  // the schema actually supports, the way the old hand-written literal
  // list did (confirmed live: it only ever covered levels 1-3).
  //
  // codeBlock isn't part of that set — BlockNote scopes its own helper to
  // block types a toolbar *retype* makes sense for — so it's appended here
  // by hand, same as before, just sharing one lookup/render path with
  // everything else applyBlockType handles. (Divider got the same
  // treatment in an earlier version of this toolbar; it now lives in the
  // Insert popover instead, via BlockNote's own insert-semantics
  // onItemClick — see INSERT_ITEM_KEYS above — so it's not here.)
  const blockTypeItems = useMemo(() => {
    const items = getDefaultBlockTypeItems(editor).map((item) => ({
      key: item.key as string,
      type: item.type,
      props: item.props,
      title: editor.dictionary.slash_menu[item.key].title,
      group: editor.dictionary.slash_menu[item.key].group,
    }));
    items.push({
      key: "code_block",
      type: "codeBlock",
      props: undefined,
      title: editor.dictionary.slash_menu.code_block.title,
      group: editor.dictionary.slash_menu.code_block.group,
    });
    return items;
  }, [editor]);

  const applyBlockType = (key: string) => {
    const item = blockTypeItems.find((i) => i.key === key);
    if (!item) return;
    const { block } = editor.getTextCursorPosition();
    // `Parameters<typeof editor.updateBlock>[1]` rather than a
    // `PartialBlock<any, any, any>` cast: the latter doesn't actually
    // satisfy `updateBlock`'s second parameter (confirmed live — TS2345,
    // since this editor's real schema type isn't `any`/`any`/`any`), while
    // this asks for whatever that parameter's type actually resolves to
    // for *this* editor instance, which a same-shaped object always
    // satisfies.
    editor.updateBlock(
      block,
      { type: item.type, props: item.props } as Parameters<typeof editor.updateBlock>[1],
    );
  };

  // Clears every mark BlockNote's default style schema has — text/
  // highlight color included now that the read view (crates/web/src/
  // wiki.rs's render_blocks_html) renders content_json directly rather
  // than a markdown export, so a color applied here actually persists.
  const clearFormatting = () => {
    editor.removeStyles({
      bold: true,
      italic: true,
      underline: true,
      strike: true,
      code: true,
      textColor: "default",
      backgroundColor: "default",
    } as Parameters<typeof editor.removeStyles>[0]);
  };

  // Block-level, not a mark — there's no such thing as mid-paragraph
  // alignment, so this retypes the current block's `textAlignment` prop
  // the same way applyBlockType retypes its `type`/other `props`.
  const applyAlignment = (alignment: "left" | "center" | "right" | "justify") => {
    const { block } = editor.getTextCursorPosition();
    editor.updateBlock(
      block,
      { props: { textAlignment: alignment } } as Parameters<typeof editor.updateBlock>[1],
    );
  };

  // `editor.addStyles`/`removeStyles` is BlockNote's own documented pattern
  // for applying a style to the current selection (see `@blocknote/react`'s
  // own `ColorStyleButton` — this mirrors it exactly) — "default" removes
  // the style rather than setting it to a color named "default", same as
  // clicking a swatch never actually sets `backgroundColor: "none"`.
  const setTextColor = (color: string) => {
    if (color === "default") {
      editor.removeStyles({ textColor: color } as Parameters<typeof editor.removeStyles>[0]);
    } else {
      editor.addStyles({ textColor: color } as Parameters<typeof editor.addStyles>[0]);
    }
  };

  const setHighlightColor = (color: string) => {
    if (color === "default") {
      editor.removeStyles({ backgroundColor: color } as Parameters<typeof editor.removeStyles>[0]);
    } else {
      editor.addStyles({ backgroundColor: color } as Parameters<typeof editor.addStyles>[0]);
    }
  };

  // Columns as a format applied to a selection, not something inserted
  // empty and filled in afterward — tried the other way first (BlockNote's
  // own insertColumnList), and it read wrong in practice: two blank
  // columns with no connection to anything already on the page, that you
  // then have to click into one at a time and retype content into. This
  // takes whatever selection is active — one highlighted paragraph is
  // enough, not just a multi-block drag — splits it into `count`
  // contiguous reading-order groups (first third of the selected blocks in
  // column one, etc. — a newspaper-style split, not round-robin), and lays
  // those out side by side in their place. getSelection() returns undefined
  // for a plain collapsed cursor — nothing selected to act on — so this is
  // a no-op then; the toolbar disables the buttons accordingly (see
  // hasColumnSelection above) rather than doing nothing silently.
  const formatSelectionAsColumns = (count: 2 | 3) => {
    const blocks = editor.getSelection()?.blocks;
    if (!blocks || blocks.length < 1) {
      return;
    }
    const replaceWithColumns = (columns: { type: "column"; children: any[] }[]) => {
      editor.insertBlocks(
        [{ type: "columnList" as const, children: columns }] as Parameters<typeof editor.insertBlocks>[0],
        blocks[0],
        "before",
      );
      editor.removeBlocks(blocks.map((b) => b.id));
    };

    // A fair, contiguous split: the first `blocks.length % count` columns
    // get one extra block each, rather than Math.ceil-ing every column's
    // share (confirmed live that overloaded the early columns for anything
    // that didn't divide evenly — 7 blocks into 3 columns gave 3/3/1, not
    // the 3/2/2 this gives).
    const base = Math.floor(blocks.length / count);
    const extra = blocks.length % count;
    let offset = 0;
    const blockGroups = Array.from({ length: count }, (_, i) => {
      const size = base + (i < extra ? 1 : 0);
      const slice = blocks.slice(offset, offset + size);
      offset += size;
      return slice;
    });

    // Fewer selected blocks than columns (most commonly: one highlighted
    // paragraph/heading) — no way to fill every column by whole blocks no
    // matter how they're grouped, so split the text itself instead (see
    // splitInlineContent above) rather than leaving columns empty. Only
    // when every selected block actually carries plain inline content to
    // split — an image/table/divider/etc. has nothing to split, so that
    // falls through to the plain-empty-column placement below exactly as
    // before this existed.
    const needsTextSplit = blockGroups.some((group) => group.length === 0);
    if (needsTextSplit && blocks.every((b) => Array.isArray(b.content))) {
      const combinedContent = blocks.flatMap((b) => b.content as any[]);
      const pieces = splitInlineContent(combinedContent, count);
      replaceWithColumns(
        pieces.map((content) => ({
          type: "column" as const,
          children: [{ type: "paragraph" as const, content }],
        })),
      );
      return;
    }

    replaceWithColumns(
      blockGroups.map((group) => ({
        type: "column" as const,
        // Still possible here (a non-text block, e.g. an image, selected
        // alongside too few siblings to fill every column) — seeded with
        // one empty paragraph, same as BlockNote's own default for a
        // freshly-inserted column, so the layout still has exactly the
        // number of columns asked for rather than silently fewer.
        children: group.length > 0 ? group.map(stripBlockIds) : [{ type: "paragraph" as const }],
      })),
    );
  };

  // The counterpart to formatSelectionAsColumns: no way to undo a column
  // layout once applied was a real gap, flagged directly — this unwraps
  // one back to plain flowing blocks. Each column's children, in column
  // order and each column's own order, becomes a sequential run of
  // top-level blocks in the columnList's former place — the same reading
  // order formatSelectionAsColumns builds columns from, run backwards.
  const removeColumns = (columnList: any) => {
    const flattened = columnList.children.flatMap((column: any) => column.children.map(stripBlockIds));
    editor.insertBlocks(
      (flattened.length > 0 ? flattened : [{ type: "paragraph" as const }]) as Parameters<
        typeof editor.insertBlocks
      >[0],
      columnList,
      "before",
    );
    editor.removeBlocks([columnList.id]);
  };

  // Table has no single "insert at cursor" shorthand the way block-type
  // swaps do — insertBlocks at the current block's position, then step
  // past the table into the paragraph BlockNote always inserts after it
  // (tables can't be the last block), so the cursor lands somewhere
  // sensible instead of inside the table's own fiddly cell-selection model.
  const insertTable = (rows: number, cols: number) => {
    const { block } = editor.getTextCursorPosition();
    editor.insertBlocks(
      [
        {
          type: "table",
          content: {
            type: "tableContent",
            rows: Array.from({ length: rows }, () => ({
              cells: Array.from({ length: cols }, () => ""),
            })),
          },
        },
      ],
      block,
      "after",
    );
  };

  const insertTableOfContents = () => {
    const { block } = editor.getTextCursorPosition();
    editor.insertBlocks([{ type: "tableOfContents" }], block, "after");
  };

  // Starts empty — FootnoteRefView opens its own popover immediately for an
  // empty footnote (see its own `open` initial state), so this is "insert,
  // then type," one motion, not insert-then-separately-click-to-open.
  const insertFootnote = () => {
    editor.insertInlineContent([
      { type: "footnoteRef", props: { noteId: crypto.randomUUID(), text: "" } },
    ]);
  };

  // Image/Video/Audio/File/Emoji — the handful of "/" menu items that
  // aren't simple block-type swaps (those live in the <select> above) or
  // already have their own toolbar button (Table). Reuses BlockNote's own
  // item list — same onItemClick as picking these from "/" itself,
  // including immediately opening the upload panel for the media types —
  // just filtered to the ones with no other home and rendered as one flat
  // list instead of "/"'s nested, scrollable category menu.
  const insertItems = useMemo(() => {
    const byKey = new Map<string, DefaultSuggestionItem>(
      getDefaultSlashMenuItems(editor).map((item) => [item.key, item]),
    );
    return INSERT_ITEM_KEYS.map((key) => byKey.get(key)).filter(
      (item): item is DefaultSuggestionItem => item !== undefined,
    );
  }, [editor]);

  return (
    <div className="wiki-editor-toolbar">
      <span className="wiki-toolbar-insert" ref={insertRef}>
        <button
          type="button"
          title="Insert — image, video, audio, file, emoji"
          className="wiki-toolbar-btn wiki-toolbar-btn-insert"
          onMouseDown={(ev) => {
            ev.preventDefault();
            setInsertOpen((open) => !open);
          }}
        >
          <IconPlus />
        </button>
        {insertOpen && (
          <span className="wiki-toolbar-insert-popover">
            {insertItems.map((item) => {
              const Icon = INSERT_ITEM_ICONS[item.key];
              return (
                <button
                  key={item.key}
                  type="button"
                  onMouseDown={(ev) => {
                    ev.preventDefault();
                    item.onItemClick();
                    setInsertOpen(false);
                  }}
                >
                  {Icon && <Icon />}
                  {item.title}
                </button>
              );
            })}
            <button
              type="button"
              onMouseDown={(ev) => {
                ev.preventDefault();
                insertTableOfContents();
                setInsertOpen(false);
              }}
            >
              <IconTableOfContents />
              Table of contents
            </button>
            <button
              type="button"
              onMouseDown={(ev) => {
                ev.preventDefault();
                insertFootnote();
                setInsertOpen(false);
              }}
            >
              <IconFootnote />
              Footnote
            </button>
          </span>
        )}
      </span>
      <span className="wiki-toolbar-sep" />
      {/* Uncontrolled, and never reset after firing: resetting back to
          "paragraph" on every change broke re-selecting "Paragraph" itself
          (same DOM value as the reset target -> no change event -> nothing
          happens), caught by hand-testing this exact toggle-back case. This
          doesn't track the current block's real type as the cursor moves
          (would need a selection-change subscription) but every selection
          here reliably fires and applies. */}
      <select
        className="wiki-toolbar-blocktype"
        defaultValue="paragraph"
        onMouseDown={(ev) => ev.stopPropagation()}
        onChange={(ev) => applyBlockType(ev.target.value)}
      >
        {/* Paragraph as a bare top-level option, same spot Confluence's own
            "Normal text" dropdown puts it (first, above the heading
            levels) — only the two heading groups below get optgroup
            labels, since Headings vs. Subheadings is the one distinction
            worth calling out visually; Paragraph is a single item with
            nothing to group it against. */}
        <option value="paragraph">Paragraph</option>
        {SELECT_GROUP_ORDER.map((group) => (
          <optgroup key={group} label={group}>
            {blockTypeItems
              .filter((item) => item.group === group)
              .map((item) => (
                <option key={item.key} value={item.key}>
                  {item.title}
                </option>
              ))}
          </optgroup>
        ))}
      </select>
      <span className="wiki-toolbar-sep" />
      {MARK_BUTTONS.map(({ icon: Icon, title, style }) => (
        <button
          key={style}
          type="button"
          title={title}
          className={activeStyles[style] ? "wiki-toolbar-btn active" : "wiki-toolbar-btn"}
          aria-pressed={Boolean(activeStyles[style])}
          onMouseDown={(ev) => {
            ev.preventDefault();
            editor.toggleStyles({ [style]: true });
          }}
        >
          <Icon />
        </button>
      ))}
      <span className="wiki-toolbar-sep" />
      <span className="wiki-toolbar-color" ref={textColorRef}>
        <button
          type="button"
          title="Text color"
          className="wiki-toolbar-btn"
          onMouseDown={(ev) => {
            ev.preventDefault();
            setTextColorOpen((open) => !open);
          }}
        >
          <IconTextColor />
        </button>
        {textColorOpen && (
          <span className="wiki-color-popover">
            <button
              type="button"
              className="wiki-color-swatch wiki-color-swatch-default"
              title="Default"
              onMouseDown={(ev) => {
                ev.preventDefault();
                setTextColor("default");
                setTextColorOpen(false);
              }}
            />
            {COLOR_NAMES.map((name) => (
              <button
                key={name}
                type="button"
                className="wiki-color-swatch"
                title={name}
                style={{ backgroundColor: COLORS_DEFAULT[name].text }}
                onMouseDown={(ev) => {
                  ev.preventDefault();
                  setTextColor(name);
                  setTextColorOpen(false);
                }}
              />
            ))}
          </span>
        )}
      </span>
      <span className="wiki-toolbar-color" ref={highlightRef}>
        <button
          type="button"
          title="Highlight color"
          className="wiki-toolbar-btn"
          onMouseDown={(ev) => {
            ev.preventDefault();
            setHighlightOpen((open) => !open);
          }}
        >
          <IconHighlight />
        </button>
        {highlightOpen && (
          <span className="wiki-color-popover">
            <button
              type="button"
              className="wiki-color-swatch wiki-color-swatch-default"
              title="None"
              onMouseDown={(ev) => {
                ev.preventDefault();
                setHighlightColor("default");
                setHighlightOpen(false);
              }}
            />
            {COLOR_NAMES.map((name) => (
              <button
                key={name}
                type="button"
                className="wiki-color-swatch"
                title={name}
                style={{ backgroundColor: COLORS_DEFAULT[name].background }}
                onMouseDown={(ev) => {
                  ev.preventDefault();
                  setHighlightColor(name);
                  setHighlightOpen(false);
                }}
              />
            ))}
          </span>
        )}
      </span>
      <span className="wiki-toolbar-sep" />
      {ALIGN_BUTTONS.map(({ icon: Icon, title, alignment }) => (
        <button
          key={alignment}
          type="button"
          title={title}
          className="wiki-toolbar-btn"
          onMouseDown={(ev) => {
            ev.preventDefault();
            applyAlignment(alignment);
          }}
        >
          <Icon />
        </button>
      ))}
      <span className="wiki-toolbar-sep" />
      {/* A format applied to a selection (see formatSelectionAsColumns
          above), grouped with alignment rather than left under "Insert" —
          it acts on blocks already selected, not something new dropped in
          at the cursor, so it needs its own trigger that can actually be
          disabled when there's nothing selected to act on. Also the one
          place to undo it (see removeColumns above) — flagged directly that
          there was previously no way back to a single column once applied,
          so the button stays enabled whenever either action has something
          to act on: a real selection to convert, or an existing layout
          the cursor is sitting in to unwrap. */}
      <span className="wiki-toolbar-columns" ref={columnsRef}>
        <button
          type="button"
          title={
            hasColumnSelection || enclosingColumnList
              ? "Columns"
              : "Columns — select some text or blocks first"
          }
          className="wiki-toolbar-btn"
          disabled={!hasColumnSelection && !enclosingColumnList}
          onMouseDown={(ev) => {
            ev.preventDefault();
            setColumnsOpen((open) => !open);
          }}
        >
          <IconColumns2 />
        </button>
        {columnsOpen && (
          <span className="wiki-toolbar-insert-popover">
            <button
              type="button"
              disabled={!hasColumnSelection}
              onMouseDown={(ev) => {
                ev.preventDefault();
                formatSelectionAsColumns(2);
                setColumnsOpen(false);
              }}
            >
              <IconColumns2 />
              2 columns
            </button>
            <button
              type="button"
              disabled={!hasColumnSelection}
              onMouseDown={(ev) => {
                ev.preventDefault();
                formatSelectionAsColumns(3);
                setColumnsOpen(false);
              }}
            >
              <IconColumns3 />
              3 columns
            </button>
            {enclosingColumnList && (
              <button
                type="button"
                onMouseDown={(ev) => {
                  ev.preventDefault();
                  removeColumns(enclosingColumnList);
                  setColumnsOpen(false);
                }}
              >
                <IconSingleColumn />
                Remove columns
              </button>
            )}
          </span>
        )}
      </span>
      <span className="wiki-toolbar-sep" />
      {BASIC_BLOCK_BUTTONS.map(({ icon: Icon, title, key }) => (
        <button
          key={key}
          type="button"
          title={title}
          className="wiki-toolbar-btn"
          onMouseDown={(ev) => {
            ev.preventDefault();
            applyBlockType(key);
          }}
        >
          <Icon />
        </button>
      ))}
      <button
        type="button"
        title="Clear formatting"
        className="wiki-toolbar-btn"
        onMouseDown={(ev) => {
          ev.preventDefault();
          clearFormatting();
        }}
      >
        <IconClearFormatting />
      </button>
      <span className="wiki-toolbar-sep" />
      {/* An inline popover, not window.prompt() — a native prompt blocks
          the whole page and looks like nothing else in this app (every
          other "type a thing, confirm it" moment here is an in-page
          field, e.g. the sidebar's page-title input). */}
      <span className="wiki-toolbar-link" ref={linkRef}>
        <button
          type="button"
          title="Link"
          className="wiki-toolbar-btn"
          onMouseDown={(ev) => {
            ev.preventDefault();
            const existing = editor.getSelectedLinkUrl();
            setLinkUrl(existing ?? "");
            setLinkText(editor.getSelectedText());
            setLinkOpen((open) => !open);
          }}
        >
          <IconLink />
        </button>
        {linkOpen && (
          <span className="wiki-toolbar-link-popover">
            <input
              type="text"
              placeholder="https://…"
              autoFocus
              value={linkUrl}
              onChange={(ev) => setLinkUrl(ev.target.value)}
              onKeyDown={(ev) => {
                if (ev.key === "Enter") {
                  ev.preventDefault();
                  if (linkUrl.trim()) editor.createLink(linkUrl.trim(), linkText || linkUrl.trim());
                  setLinkOpen(false);
                } else if (ev.key === "Escape") {
                  setLinkOpen(false);
                }
              }}
            />
            <button
              type="button"
              onMouseDown={(ev) => {
                ev.preventDefault();
                if (linkUrl.trim()) editor.createLink(linkUrl.trim(), linkText || linkUrl.trim());
                setLinkOpen(false);
              }}
            >
              Apply
            </button>
          </span>
        )}
      </span>
      <span className="wiki-toolbar-table" ref={tableRef}>
        <button
          type="button"
          title="Table"
          className="wiki-toolbar-btn"
          onMouseDown={(ev) => {
            ev.preventDefault();
            setTableOpen((open) => !open);
            setTableHover(null);
          }}
        >
          <IconTable />
        </button>
        {tableOpen && (
          <span className="wiki-table-picker">
            <span className="wiki-table-picker-label">
              {tableHover ? `${tableHover.row} × ${tableHover.col} table` : "Insert table"}
            </span>
            <span
              className="wiki-table-picker-grid"
              style={{
                gridTemplateColumns: `repeat(${TABLE_PICKER_SIZE}, 1fr)`,
                gridTemplateRows: `repeat(${TABLE_PICKER_SIZE}, 1fr)`,
              }}
              onMouseLeave={() => setTableHover(null)}
            >
              {Array.from({ length: TABLE_PICKER_SIZE * TABLE_PICKER_SIZE }, (_, i) => {
                const row = Math.floor(i / TABLE_PICKER_SIZE) + 1;
                const col = (i % TABLE_PICKER_SIZE) + 1;
                const active = tableHover !== null && row <= tableHover.row && col <= tableHover.col;
                return (
                  <span
                    key={i}
                    className={active ? "wiki-table-picker-cell active" : "wiki-table-picker-cell"}
                    onMouseEnter={() => setTableHover({ row, col })}
                    onMouseDown={(ev) => {
                      ev.preventDefault();
                      insertTable(row, col);
                      setTableOpen(false);
                      setTableHover(null);
                    }}
                  />
                );
              })}
            </span>
          </span>
        )}
      </span>
      <button
        type="button"
        title={
          tableCellAction?.mergeDirection
            ? `Merge cells (${tableCellAction.mergeDirection})`
            : "Merge cells — select more than one cell first"
        }
        className="wiki-toolbar-btn"
        disabled={!tableCellAction?.mergeDirection}
        onMouseDown={(ev) => {
          ev.preventDefault();
          editor.getExtension(TableHandlesExtension)?.mergeCells();
        }}
      >
        <IconMergeCells />
      </button>
      <button
        type="button"
        title={
          tableCellAction?.canSplit
            ? "Split cell"
            : "Split cell — place the cursor in a merged cell first"
        }
        className="wiki-toolbar-btn"
        disabled={!tableCellAction?.canSplit}
        onMouseDown={(ev) => {
          ev.preventDefault();
          editor.getExtension(TableHandlesExtension)?.splitCell();
        }}
      >
        <IconSplitCells />
      </button>
    </div>
  );
}

// The narrow boundary between Rust and this island: a handle with exactly
// the operations wiki_editor.rs needs (mount/read/replace/destroy), kept on
// `window` since wasm_bindgen reaches it as a plain JS object, not an ES
// module import — see the mount/getDocument/getMarkdown/loadMarkdown/destroy
// plan from the editor-tooling session notes.
export interface WikiEditorHandle {
  // `Block<any, any, any>`, not the schema-specific `Block` this file
  // otherwise uses: this value crosses straight into
  // `window.getWikiEditorDocument`'s `JSON.stringify`, so nothing here
  // actually depends on its precise shape.
  getDocument: () => Block<any, any, any>[];
  getMarkdown: () => Promise<string>;
  loadMarkdown: (markdown: string) => Promise<void>;
  destroy: () => void;
}

declare global {
  interface Window {
    mountWikiEditor?: (
      elementId: string,
      initialDocumentJson: string | null,
      readOnly: boolean,
    ) => void;
    unmountWikiEditor?: (elementId: string) => void;
    // Flat wrapper functions, not direct access to __wikiEditors[id] — a
    // wasm_bindgen extern block binds a handful of named globals far more
    // simply than reaching through a JS object property by string key.
    getWikiEditorDocument?: (elementId: string) => string;
    getWikiEditorMarkdown?: (elementId: string) => Promise<string>;
    loadWikiEditorMarkdown?: (elementId: string, markdown: string) => Promise<void>;
    __wikiEditors?: Record<string, WikiEditorHandle>;
  }
}

function requireHandle(elementId: string): WikiEditorHandle {
  const handle = window.__wikiEditors?.[elementId];
  if (!handle) throw new Error(`no wiki editor mounted at "${elementId}"`);
  return handle;
}

// Pages saved before the tableOfContents block type existed (see
// TableOfContentsBlock above) stored a plain paragraph containing the
// literal text "[TOC]" instead — upgraded here, the moment such a page is
// next opened for editing, rather than left as inert text forever. Once
// upgraded the editor's own document differs from what was loaded, so the
// normal autosave/Done-flush dirty-check (content_json vs. last-saved, in
// app.rs) persists the upgrade like any other edit, with no separate
// migration path needed. crates/web/src/wiki.rs's with_table_of_contents
// still recognizes the old marker too, for a page that hasn't been
// reopened for editing yet.
function upgradeLegacyTocMarkers<T extends Block<any, any, any>>(blocks: T[]): T[] {
  return blocks.map((block) => {
    const children = block.children?.length ? upgradeLegacyTocMarkers(block.children) : block.children;
    const isLegacyMarker =
      block.type === "paragraph" &&
      Array.isArray(block.content) &&
      block.content.length === 1 &&
      (block.content[0] as { type?: string } | undefined)?.type === "text" &&
      (block.content[0] as { text?: string } | undefined)?.text === "[TOC]";
    if (isLegacyMarker) {
      return { id: block.id, type: "tableOfContents", props: {}, content: [], children } as unknown as T;
    }
    return children === block.children ? block : { ...block, children };
  });
}

function parseInitialContent(json: string | null): Block<any, any, any>[] | undefined {
  if (!json) return undefined;
  try {
    const blocks = JSON.parse(json) as Block<any, any, any>[];
    // A brand-new page's first revision stores '{}' (see db.rs's
    // create_page), not a block array — BlockNote wants undefined (its own
    // default empty document) rather than an empty/invalid array here.
    return Array.isArray(blocks) && blocks.length > 0 ? upgradeLegacyTocMarkers(blocks) : undefined;
  } catch {
    return undefined;
  }
}

function WikiEditorApp({
  initialDocumentJson,
  readOnly,
  onReady,
}: {
  initialDocumentJson: string | null;
  readOnly: boolean;
  onReady: (handle: WikiEditorHandle) => void;
}) {
  const initialContent = useMemo(() => parseInitialContent(initialDocumentJson), []);
  const [importError, setImportError] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const editor = useCreateBlockNote({
    schema: wikiSchema,
    initialContent,
    // Only splitCells — not headers/cellBackgroundColor/cellTextColor,
    // BlockNote's other opt-in table features: this app only asked for
    // merge/split parity with Confluence/Google Docs, not those. Split
    // itself is already reachable from each cell's own hover handle (see
    // TableHandles, on by default); this flag is what turns that on and
    // what the Toolbar's Merge/Split buttons below check via
    // editor.settings.tables.splitCells.
    tables: { splitCells: true },
  });

  const handleReadyRef = useRef(onReady);
  handleReadyRef.current = onReady;

  useMemo(() => {
    handleReadyRef.current({
      getDocument: () => editor.document,
      // Wrapped in Promise.resolve regardless of whether this BlockNote
      // version's blocksToMarkdownLossy is sync or async (confirmed live:
      // it's actually sync here, returning a plain string) — the Rust side
      // always awaits a real Promise via JsFuture, which throws "arg0.then
      // is not a function" on a bare string.
      getMarkdown: () => Promise.resolve(editor.blocksToMarkdownLossy()),
      loadMarkdown: async (markdown: string) => {
        const blocks = await editor.tryParseMarkdownToBlocks(markdown);
        editor.replaceBlocks(editor.document, blocks);
      },
      destroy: () => {
        /* no editor.destroy() in the current API — unmounting the React
           root (see unmountWikiEditor below) is the actual teardown. */
      },
    });
    // editor is stable for the component's lifetime (useCreateBlockNote
    // doesn't recreate it on rerender), so this only needs to run once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editor]);

  const importFile = useCallback(
    async (file: File) => {
      setImportError(null);
      if (!file.name.toLowerCase().endsWith(".md") && file.type !== "text/markdown") {
        setImportError("That doesn't look like a markdown (.md) file.");
        return;
      }
      const text = await file.text();
      const blocks = await editor.tryParseMarkdownToBlocks(text);
      editor.replaceBlocks(editor.document, blocks);
    },
    [editor],
  );

  return (
    <div
      className="wiki-editor-shell"
      onDragOver={(ev) => {
        ev.preventDefault();
      }}
      onDrop={(ev) => {
        ev.preventDefault();
        const file = ev.dataTransfer.files[0];
        if (file) void importFile(file);
      }}
    >
      {!readOnly && (
        <>
          <Toolbar editor={editor} />
          <div className="wiki-editor-secondary-row">
            <button
              type="button"
              className="wiki-editor-import-button"
              title="Import a markdown file"
              onMouseDown={(ev) => ev.preventDefault()}
              onClick={() => fileInputRef.current?.click()}
            >
              Import .md
            </button>
            <span className="wiki-editor-import-hint">or drop a .md file onto the page</span>
          </div>
          <input
            ref={fileInputRef}
            type="file"
            accept=".md,text/markdown"
            style={{ display: "none" }}
            onChange={(ev) => {
              const file = ev.target.files?.[0];
              if (file) void importFile(file);
              ev.target.value = "";
            }}
          />
          {importError && <span className="wiki-editor-import-error">{importError}</span>}
        </>
      )}
      {/* theme="light" rather than following the OS/browser preference
          (Mantine's default): the editor sits inside the same cream "index
          card" surface as the read view, and picking up a dark theme made
          editing look like an entirely different, disconnected component
          instead of the same page becoming interactive.
          formattingToolbar={false}: replaced by the always-visible Toolbar
          above — BlockNote's own formatting controls only ever appear in a
          popup on text selection, which is exactly the "obscure" complaint
          this whole change is addressing; having both would just be two
          competing toolbars.
          sideMenu={false}: the floating drag-handle/"+" column BlockNote
          shows to the left of whichever block you're hovering — flagged
          directly as unwanted; the toolbar's block-type dropdown and
          plain Enter-at-end-of-block already cover adding/changing
          blocks, so this wasn't the only way to do either. */}
      <BlockNoteView
        editor={editor}
        editable={!readOnly}
        theme="light"
        formattingToolbar={false}
        sideMenu={false}
      />
    </div>
  );
}

const roots: Record<string, Root> = {};

window.mountWikiEditor = (
  elementId: string,
  initialDocumentJson: string | null,
  readOnly: boolean,
) => {
  const el = document.getElementById(elementId);
  if (!el) {
    throw new Error(`mountWikiEditor: no element with id "${elementId}"`);
  }
  window.__wikiEditors ??= {};
  // Genuinely idempotent, not just documented as such (see wiki_editor.rs's
  // own mount() comment, which already assumed this): calling createRoot()
  // again on an element that already has a root tears down and replaces
  // its whole DOM subtree. The actual repeated-mount bug this was guarding
  // against is now fixed at its source (app.rs's `page` Resource no longer
  // refetches — and so no longer rebuilds <WikiEditor> — on every autosave;
  // see its own comment), but this stays as a cheap second line of defense:
  // a second mount call for any other reason now just re-renders onto the
  // existing root instead of destroying and recreating the DOM, so it
  // can't blow away whatever selection/cursor position or focus was live.
  // initialDocumentJson is still only ever applied once regardless
  // (WikiEditorApp's own useMemo has an empty dep array).
  const root = roots[elementId] ?? createRoot(el);
  roots[elementId] = root;
  root.render(
    <WikiEditorApp
      initialDocumentJson={initialDocumentJson}
      readOnly={readOnly}
      onReady={(handle) => {
        window.__wikiEditors![elementId] = handle;
      }}
    />,
  );
};

window.unmountWikiEditor = (elementId: string) => {
  roots[elementId]?.unmount();
  delete roots[elementId];
  if (window.__wikiEditors) delete window.__wikiEditors[elementId];
};

window.getWikiEditorDocument = (elementId: string) =>
  JSON.stringify(requireHandle(elementId).getDocument());

window.getWikiEditorMarkdown = (elementId: string) => requireHandle(elementId).getMarkdown();

window.loadWikiEditorMarkdown = (elementId: string, markdown: string) =>
  requireHandle(elementId).loadMarkdown(markdown);
