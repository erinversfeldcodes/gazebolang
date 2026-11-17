# gazebolang

> The Order of the Gazebo: a flashcard app for learning isiXhosa. Afrikaans, Spanish, German and French are pending.

gazebolang is a web app for building and reviewing flashcards in a language you are learning. It has cards and decks, spaced-repetition scheduling (FSRS), several trainers (spelling, sound to spelling, minimal pairs and back-chaining), a dictionary, a wiki with its own editor, voice recording, and image search and upload for image cards. A separate command-line tool, `importer`, reads photographed dictionary pages on your own machine and creates cards from them on a running instance.

> **Status: in active development, and used by a small number of people.** Accounts are by invitation. The source is publicly visible under a restrictive licence: you may read it, but may not use, copy or distribute it. See [LICENSE](./LICENSE).

## How it is built

- **`crates/web`**: the app. Rust, with [Leptos](https://leptos.dev) (server-rendered and hydrated in the browser as WebAssembly) on [Axum](https://github.com/tokio-rs/axum), and [sqlx](https://github.com/launchbadge/sqlx) on PostgreSQL. Migrations are in `migrations/`.
- **`crates/domain`**: the spaced-repetition scheduling (`srs`), which does not depend on the web.
- **`crates/importer`**: the dictionary-page importer. It runs a vision model locally through `llama.cpp`, with no cloud API, so a first build compiles `llama.cpp` and takes a long time. See its own documentation for usage.
- **`e2e/`**: end-to-end tests, with [Playwright](https://playwright.dev), run against a local instance in CI and against a preview deploy.
- **Deployment**: [Fly.io](https://fly.io) (`fly.toml`), with a preview app and a [Neon](https://neon.tech) database branch for each pull request. Card images go to S3-compatible storage; locally that is [Garage](https://garagehq.deuxfleurs.fr).

## Running it locally

You need Rust (the exact version is pinned in `rust-toolchain.toml`, so use `rustup`), [`just`](https://just.systems), Docker with Compose, `cargo-leptos` and `sqlx-cli`. On macOS, `brew bundle install` installs the host tools, and `nix develop` provides the rest; see the `Brewfile` and `flake.nix`.

```
cp .env.example .env     # then read it: most of it is optional
just db-up               # PostgreSQL and Garage, in Docker
just storage-init        # one-time Garage bootstrap
just db-migrate          # apply the migrations
just dev                 # cargo leptos watch, at http://127.0.0.1:3000
```

`just ci` runs the formatting check, the lints and the tests, as CI does. Some tests need a database (`DATABASE_URL` in `.env`), and one calls the real Pixabay API and needs a `PIXABAY_API_KEY`.

## Contributing

Issues are welcome, and pull requests from outside contributors are not accepted. See [CONTRIBUTING.md](./CONTRIBUTING.md). To report a vulnerability, see [SECURITY.md](./SECURITY.md).

## Licence

See [LICENSE](./LICENSE).
