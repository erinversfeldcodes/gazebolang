//! The dictionary: every word that has at least one flashcard, browsable
//! English → target language or target language → English, with a detail
//! page per word that links to all of its cards.
use crate::app::CardPager;
use crate::cards::{CARD_TYPES, CARDS_PAGE_SIZE, CARDS_PAGE_SIZE_OPTIONS};
use crate::dictionary::{get_dictionary_entry, list_dictionary};
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use uuid::Uuid;

fn card_type_label(card_type: &str) -> &'static str {
    CARD_TYPES
        .iter()
        .find(|(value, _)| *value == card_type)
        .map_or("Card", |(_, label)| *label)
}

#[component]
// `#[component]` doesn't forward `#[must_use]`; see `RecordButton` in
// media_recorder.rs for the same allow.
#[allow(clippy::too_many_lines, clippy::must_use_candidate)]
pub(crate) fn DictionaryPage() -> impl IntoView {
    // true: English is the headword and the sort key; false: the target
    // language is.
    let english_first = RwSignal::new(true);
    let search_box = RwSignal::new(String::new());
    // Applied on change/submit rather than per keystroke, so typing doesn't
    // fire a query for every character.
    let search = RwSignal::new(String::new());
    let page = RwSignal::new(0_i64);
    let page_size = RwSignal::new(CARDS_PAGE_SIZE);

    let entries = Resource::new(
        move || {
            (
                english_first.get(),
                search.get(),
                page.get(),
                page_size.get(),
            )
        },
        |(english_first, search, page, page_size)| {
            list_dictionary(english_first, search, page, page_size)
        },
    );

    view! {
        <h1>"Dictionary"</h1>
        <p class="page-intro">
            "Every word that has a flashcard. Open a word to see its details and all of its cards."
        </p>
        <div class="card-filters">
            <span class="dictionary-direction" role="group" aria-label="Direction">
                <button
                    type="button"
                    aria-pressed=move || english_first.get().to_string()
                    class:active=move || english_first.get()
                    on:click=move |_| {
                        english_first.set(true);
                        page.set(0);
                    }
                >
                    "English → target"
                </button>
                <button
                    type="button"
                    aria-pressed=move || (!english_first.get()).to_string()
                    class:active=move || !english_first.get()
                    on:click=move |_| {
                        english_first.set(false);
                        page.set(0);
                    }
                >
                    "Target → English"
                </button>
            </span>
            " "
            <label>
                "Search "
                <input
                    type="search"
                    prop:value=move || search_box.get()
                    on:input=move |ev| search_box.set(event_target_value(&ev))
                    on:change=move |_| {
                        search.set(search_box.get_untracked());
                        page.set(0);
                    }
                />
            </label>
            " "
            <label>
                "Words per page "
                <select
                    on:change=move |ev| {
                        if let Ok(new_size) = event_target_value(&ev).parse::<i64>() {
                            let first_item = page.get_untracked() * page_size.get_untracked();
                            page_size.set(new_size);
                            page.set(first_item / new_size);
                        }
                    }
                >
                    {CARDS_PAGE_SIZE_OPTIONS.into_iter().map(|n| view! {
                        <option value=n.to_string() selected=move || page_size.get() == n>
                            {n.to_string()}
                        </option>
                    }).collect_view()}
                </select>
            </label>
        </div>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                match entries.await {
                    Ok(result) if result.entries.is_empty() => {
                        view! { <p>"No words match."</p> }.into_any()
                    }
                    Ok(result) => {
                        let total = result.total;
                        let size = page_size.get_untracked();
                        let current_page = page.get_untracked();
                        let total_pages = ((total + size - 1) / size).max(1);
                        let english_first = english_first.get_untracked();
                        view! {
                            <CardPager page current_page total_pages total size />
                            <ul class="dictionary-list">
                                {result.entries.into_iter().map(|entry| {
                                    let href = format!("/dictionary/{}", entry.id);
                                    let english = entry.english_translation.unwrap_or_default();
                                    let (headword, other) = if english_first {
                                        (english, entry.target_text)
                                    } else {
                                        (entry.target_text, english)
                                    };
                                    let cards = if entry.card_count == 1 {
                                        "1 card".to_string()
                                    } else {
                                        format!("{} cards", entry.card_count)
                                    };
                                    view! {
                                        <li>
                                            <A href=href>
                                                <span class="dictionary-headword">{headword}</span>
                                                <span class="dictionary-other">{other}</span>
                                            </A>
                                            <span class="dictionary-meta">
                                                {entry.part_of_speech.unwrap_or_default()}
                                            </span>
                                            <span class="dictionary-meta">{cards}</span>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                            <CardPager page current_page total_pages total size />
                        }
                        .into_any()
                    }
                    Err(e) => view! { <p>"Error loading the dictionary: " {e.to_string()}</p> }
                        .into_any(),
                }
            })}
        </Transition>
    }
}

#[component]
// `#[component]` doesn't forward `#[must_use]`; see `RecordButton` in
// media_recorder.rs for the same allow.
#[allow(clippy::too_many_lines, clippy::must_use_candidate)]
pub(crate) fn DictionaryEntryPage() -> impl IntoView {
    let params = use_params_map();
    let word_id = move || {
        params
            .read()
            .get("id")
            .and_then(|id| Uuid::parse_str(&id).ok())
    };
    let entry = Resource::new(word_id, |id| async move {
        match id {
            Some(id) => get_dictionary_entry(id).await,
            None => Ok(None),
        }
    });

    view! {
        <p><A href="/dictionary">"← Dictionary"</A></p>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                match entry.await {
                    Ok(None) => view! { <p>"Word not found."</p> }.into_any(),
                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                    Ok(Some(entry)) => {
                        let card_count = entry.cards.len();
                        view! {
                            <h1>{entry.target_text.clone()}</h1>
                            {entry.english_translation.map(|english| view! {
                                <p class="dictionary-gloss">{english}</p>
                            })}
                            {entry.part_of_speech.map(|pos| view! {
                                <p class="dictionary-meta">{pos}</p>
                            })}
                            {entry.notes.map(|notes| view! { <p>{notes}</p> })}
                            {(!entry.examples.is_empty()).then(|| view! {
                                <h2>"Examples"</h2>
                                <ul class="dictionary-examples">
                                    {entry.examples.into_iter().map(|example| view! {
                                        <li>
                                            <span>{example.target_sentence}</span>
                                            {example.gloss_sentence.map(|gloss| view! {
                                                <span class="dictionary-meta">{gloss}</span>
                                            })}
                                        </li>
                                    }).collect_view()}
                                </ul>
                            })}
                            {(!entry.linked_words.is_empty()).then(|| view! {
                                <h2>"Related words"</h2>
                                <ul class="dictionary-related">
                                    {entry.linked_words.into_iter().map(|link| {
                                        let href = format!("/dictionary/{}", link.id);
                                        view! {
                                            <li>
                                                <A href=href>{link.target_text}</A>
                                                " "
                                                <span class="dictionary-meta">{link.link_type}</span>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            })}
                            <h2>{if card_count == 1 { "Card".to_string() } else { format!("Cards ({card_count})") }}</h2>
                            {entry.cards.is_empty().then(|| view! {
                                <p>"No cards for this word yet."</p>
                            })}
                            <ul class="dictionary-cards">
                                {entry.cards.into_iter().map(|card| {
                                    let href = format!("/cards/{}", card.id);
                                    let label = card_type_label(&card.card_type);
                                    view! {
                                        <li>
                                            <A href=href>
                                                {card.image_url.map(|url| view! { <img src=url alt="" /> })}
                                                <span>{label}</span>
                                                {card.two_way.then(|| view! {
                                                    <span class="dictionary-meta">"two-way"</span>
                                                })}
                                            </A>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }
                        .into_any()
                    }
                }
            })}
        </Transition>
    }
}
