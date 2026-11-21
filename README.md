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

### Running the end-to-end specs locally

This follows the `E2E (local)` job in `.github/workflows/ci.yml`. Do the steps in order.

1. **Install** what [Running it locally](#running-it-locally) lists, plus Node.js (CI uses version 26) with npm, and `cmake` (the importer builds `llama.cpp` from source).
2. **Set up the environment.** Copy `.env.example` to `.env` and uncomment the `S3_*` lines (`S3_BUCKET`, `S3_ENDPOINT`, `S3_ACCESS_KEY`, `S3_SECRET_KEY`, `S3_PUBLIC_URL`, `S3_PATH_STYLE`, `S3_REGION`); the specs that upload card images need storage. `image-search.spec.ts` has two tests that call the real Pixabay API and need `PIXABAY_API_KEY`.
3. **Start PostgreSQL and Garage, and migrate:** `just db-up`, `just storage-init`, `just db-migrate`.
4. **Build the wiki editor.** The wiki specs time out without it: `cd editor && npm ci && npm run build`.
5. **Build the app and the importer:** `cargo leptos build`, then `cargo build -p importer`. Two specs run `target/debug/importer`, and the `seed` project in `e2e/playwright.config.ts` uses it too.
6. **Start the app** and leave it running. CI runs `./target/debug/web` after the build, with the variables from `.env` set in its environment. Wait until `http://127.0.0.1:3000/health` answers.
7. **Install the test dependencies:** `cd e2e && npm ci && npx playwright install --with-deps chromium`.
8. **Run the specs** from `e2e/`: `npm test` (the same as `npx playwright test`, which CI runs), or `npm run test:ui` for Playwright's UI.

Environment variables that matter:

- `BASE_URL`: where the specs look for the app. Default `http://127.0.0.1:3000`, so leave it unset for a local instance.
- `DATABASE_URL`: read directly by the specs (to seed and clean up data), so it must point at the same database the app uses. The `.env.example` value is `postgres://app:app@localhost:55432/app`. CI exports it in the shell, and nothing in `e2e/` says that Playwright reads `.env`, so export it in the shell you run the specs from.
- The `S3_*` variables and `PIXABAY_API_KEY` are read by the app, so they must be set where the app is started. `PIXABAY_API_KEY` also matters to the two real-provider tests. `RUST_LOG=info` is set in CI so the app's logs show failures, and is optional.

Not settled by the CI job or the config: CI starts the built `./target/debug/web` rather than `just dev` (`cargo leptos watch`), and the repository does not say whether the specs also work against `just dev`. Likewise, CI sets `BASE_URL` explicitly, but the config default is the same address.

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
