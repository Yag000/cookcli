# CookCLI Server — Technical Architecture

This document describes, in depth, how the CookCLI web server works: which files are responsible for what, how data flows through the system, and how every major subsystem is implemented.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Technology Stack](#2-technology-stack)
3. [Source-file Map](#3-source-file-map)
4. [Server Startup](#4-server-startup)
   - 4.1 [Command-line arguments](#41-command-line-arguments)
   - 4.2 [Startup sequence](#42-startup-sequence)
   - 4.3 [Shared state — `AppState`](#43-shared-state--appstate)
5. [Router & Middleware](#5-router--middleware)
   - 5.1 [Middleware stack](#51-middleware-stack)
   - 5.2 [Complete route table](#52-complete-route-table)
6. [API Handlers](#6-api-handlers)
   - 6.1 [Recipe handlers](#61-recipe-handlers-srcserverhandlersrecipesrs)
   - 6.2 [Shopping-list handlers](#62-shopping-list-handlers-srcserverhandlersshopping_listrs)
   - 6.3 [Pantry handlers](#63-pantry-handlers-srcserverhandlerspantryrs)
   - 6.4 [Menu handlers](#64-menu-handlers-srcserverhandlersmenusrs)
   - 6.5 [Stats handler](#65-stats-handler-srcserverhandlersstatsrs)
   - 6.6 [Sync handlers](#66-sync-handlers-srcserverhandlerssyncrs-feature-gated)
7. [UI Page Handlers](#7-ui-page-handlers-srcserveruirs)
8. [HTML Templates](#8-html-templates)
   - 8.1 [Template engine](#81-template-engine)
   - 8.2 [Template data structures](#82-template-data-structures-srcservertemplatesrs)
   - 8.3 [Template files](#83-template-files)
9. [Static-file Serving](#9-static-file-serving)
10. [Shopping-list Persistence](#10-shopping-list-persistence-srcservershopping_list_storers)
11. [Internationalisation (i18n)](#11-internationalisation-i18n)
    - 11.1 [Language detection](#111-language-detection-srcserverlanguagers)
    - 11.2 [Translation lookup](#112-translation-lookup-srcserveri18nrs)
12. [LSP Bridge](#12-lsp-bridge-srcserverlsp_bridgers)
13. [Sync Subsystem](#13-sync-subsystem-srcserversync)
14. [Error Handling](#14-error-handling)
15. [Data-flow Diagrams](#15-data-flow-diagrams)

---

## 1. Overview

`cook server` starts a local HTTP/WebSocket server that exposes:

- A **browser-rendered web UI** (server-side HTML with Askama templates + Tailwind CSS)
- A **JSON REST API** consumed both by JavaScript running in the browser and by external tooling
- A **WebSocket endpoint** that bridges the browser editor to a Cooklang Language Server (LSP) subprocess

The server reads recipes directly from the filesystem on every request — there is no in-memory cache or database. Configuration (aisle and pantry) is read from TOML/conf files; the shopping list is persisted to a tab-separated text file inside the recipe directory.

---

## 2. Technology Stack

| Layer | Library | Purpose |
|---|---|---|
| Async runtime | `tokio` | Drives all async I/O |
| HTTP framework | `axum` | Routing, extractors, middleware |
| Template engine | `askama` | Compile-time HTML templates |
| CSS | Tailwind CSS | Utility-first styling (compiled to `static/css/styles.css`) |
| Static embedding | `rust-embed` | Bundles `static/` into the binary |
| File-system serving | `tower-http ServeDir` | Serves recipe images/assets at `/api/static/` |
| CORS | `tower-http CorsLayer` | Permissive CORS for API |
| Internationalisation | `fluent-templates` | Runtime translation lookup |
| Path handling | `camino` | UTF-8-typed `PathBuf` |
| Serialisation | `serde` + `serde_json` | All JSON request/response bodies |
| Recipe parsing | `cooklang` (workspace) | Parse `.cook` / `.menu` files |
| Recipe discovery | `cooklang-find` (workspace) | Directory scanning and search |

---

## 3. Source-file Map

```
src/server/
├── mod.rs               # Entry point: ServerArgs, AppState, router construction, startup
├── ui.rs                # HTML page handlers (return rendered HTML)
├── templates.rs         # Askama template structs + data types + translation helper
├── shopping_list_store.rs # Read/write .shopping_list.txt
├── language.rs          # Axum middleware: detect user language from cookie/header
├── i18n.rs              # fluent-templates loader (locales/ directory)
├── lsp_bridge.rs        # WebSocket ↔ LSP subprocess bridge
├── handlers/
│   ├── mod.rs           # Re-exports; shared error helpers
│   ├── common.rs        # Shared extractor types, path validation
│   ├── recipes.rs       # /api/recipes/* endpoints
│   ├── shopping_list.rs # /api/shopping_list/* endpoints
│   ├── pantry.rs        # /api/pantry/* endpoints
│   ├── menus.rs         # /api/menus/* endpoints
│   ├── stats.rs         # /api/stats endpoint
│   └── sync.rs          # /api/sync/* endpoints (feature = "sync")
└── sync/                # Optional cloud-sync subsystem
    ├── mod.rs
    ├── endpoints.rs
    ├── runner.rs
    └── session.rs

templates/               # Askama template files (HTML)
│   base.html            # Master layout: header, nav, footer, JS/CSS includes
│   recipes.html         # Recipe browser / directory listing
│   recipe.html          # Single recipe view
│   edit.html            # In-browser recipe editor (uses LSP WS)
│   new.html             # Create-new-recipe form
│   menu.html            # Menu file viewer
│   shopping_list.html   # Shopping list page
│   pantry.html          # Pantry inventory page
│   preferences.html     # Settings / config paths / sync status
│   error.html           # Generic error page

static/
│   css/
│   │   styles.css         # Compiled Tailwind output (main stylesheet)
│   │   cooking-mode.css   # Styles for cooking mode
│   │   input.css          # Custom component classes (source for Tailwind)
│   │   custom-styles.css  # Minor overrides
│   js/
│       cooking-mode.js    # Cooking mode toggle and step tracking
│       keyboard-shortcuts.js # Keyboard navigation
│       src/
│           editor.js      # Recipe editor initialisation
│           cooklang-mode.js # CodeMirror syntax highlighting for Cooklang
```

---

## 4. Server Startup

### 4.1 Command-line arguments

Defined in `src/server/mod.rs` as the `ServerArgs` struct (derived via `clap`):

```
cook server [BASE_PATH] [--host [<ADDRESS>]] [-p <PORT>] [--open]
```

| Flag | Type | Default | Description |
|---|---|---|---|
| `BASE_PATH` | `Option<Utf8PathBuf>` | current dir | Root directory for `.cook` files |
| `--host [<ADDRESS>]` | `Option<Option<IpAddr>>` | `None` | `None` → localhost; `Some(None)` → all interfaces (`::`); `Some(Some(ip))` → bind to `ip` |
| `-p / --port` | `u16` | `9080` | TCP port |
| `--open` | `bool` | `false` | Open browser automatically after startup |

### 4.2 Startup sequence

`pub fn run(ctx: &Context, args: ServerArgs) -> Result<()>` in `src/server/mod.rs`:

```
1. Resolve BASE_PATH to an absolute UTF-8 path
2. Build AppState
   a. Locate aisle.conf  (BASE_PATH/config/aisle.conf → ~/.config/cook/aisle.conf)
   b. Locate pantry.conf (BASE_PATH/config/pantry.conf → ~/.config/cook/pantry.conf)
3. [sync feature] Restore saved sync session (if user was previously logged in)
4. Determine bind address:
   --host not given  → 127.0.0.1 (localhost)
   --host            → :: (all IPv6/IPv4 interfaces)
   --host <IP>       → the provided IP
5. Bind TcpListener on <address>:<port>
6. Build Axum router (see §5)
7. [--open] Spawn background task that opens browser URL
8. Start axum::serve() with graceful shutdown on CTRL-C / SIGTERM
```

Maximum request body size is hard-coded to **1 MB**.

### 4.3 Shared state — `AppState`

`AppState` is wrapped in `Arc` and injected into every handler via Axum's `State` extractor:

```rust
pub struct AppState {
    pub base_path: Utf8PathBuf,               // Root recipe directory
    pub aisle_path: Option<Utf8PathBuf>,      // Path to aisle.conf (may be absent)
    pub pantry_path: Option<Utf8PathBuf>,     // Path to pantry.conf (may be absent)

    // --- only compiled when feature = "sync" ---
    pub sync_session: Arc<Mutex<Option<SyncSession>>>,
    pub sync_handle: Arc<tokio::sync::Mutex<Option<SyncHandle>>>,
    pub login_in_progress: Arc<AtomicBool>,
    pub session_path: std::path::PathBuf,
    pub shutdown_token: CancellationToken,
}
```

`base_path` is the only field needed for most handlers; the optional paths degrade gracefully when absent.

---

## 5. Router & Middleware

### 5.1 Middleware stack

Applied to every request (innermost first):

1. **`DefaultBodyLimit::max(1_048_576)`** — reject bodies > 1 MB
2. **`LanguageMiddleware`** — detect user language from `lang` cookie or `Accept-Language` header; stores `LanguageIdentifier` in request extensions
3. **`CorsLayer`** — allows all origins, methods GET / POST / PUT / DELETE, all headers (suitable for localhost use)

Image assets from the recipe directory are served via Tower's `ServeDir` at `/api/static/`, bypassing the middleware above.

### 5.2 Complete route table

```
GET  /                          → ui::recipes_page
GET  /directory/*path           → ui::recipes_directory
GET  /recipe/*path              → ui::recipe_page
GET  /edit/*path                → ui::edit_page
GET  /new                       → ui::new_page
POST /new                       → ui::create_recipe
GET  /menu/*path                → ui::menu_page_handler
GET  /shopping-list             → ui::shopping_list_page
GET  /pantry                    → ui::pantry_page
GET  /preferences               → ui::preferences_page

GET  /api/recipes               → handlers::recipes::all_recipes
GET  /api/recipes/*path         → handlers::recipes::recipe          (?scale=<f64>)
PUT  /api/recipes/*path         → handlers::recipes::recipe_save
DEL  /api/recipes/*path         → handlers::recipes::recipe_delete
GET  /api/recipes/raw/*path     → handlers::recipes::recipe_raw

GET  /api/search                → handlers::recipes::search           (?q=<string>)

POST /api/shopping_list         → handlers::shopping_list::shopping_list
GET  /api/shopping_list/items   → handlers::shopping_list::get_shopping_list_items
POST /api/shopping_list/add     → handlers::shopping_list::add_to_shopping_list
POST /api/shopping_list/remove  → handlers::shopping_list::remove_from_shopping_list
POST /api/shopping_list/clear   → handlers::shopping_list::clear_shopping_list

GET  /api/menus                 → handlers::menus::list_menus
GET  /api/menus/*path           → handlers::menus::get_menu            (?scale=<f64>)

GET  /api/pantry                → handlers::pantry::get_pantry
POST /api/pantry/add            → handlers::pantry::add_item
PUT  /api/pantry/:section/:name → handlers::pantry::update_item
DEL  /api/pantry/:section/:name → handlers::pantry::remove_item
GET  /api/pantry/expiring       → handlers::pantry::get_expiring       (?days=<i64>)
GET  /api/pantry/depleted       → handlers::pantry::get_depleted

GET  /api/stats                 → handlers::stats::stats

GET  /api/reload                → handlers::recipes::reload
POST /api/reload                → handlers::recipes::reload

WS   /api/ws/lsp                → lsp_bridge::lsp_websocket

# Sync (feature = "sync" only)
GET  /api/sync/status           → handlers::sync::sync_status
POST /api/sync/login            → handlers::sync::sync_login
POST /api/sync/logout           → handlers::sync::sync_logout

# Static assets
GET  /static/*file              → serve_static (RustEmbed — compiled-in)
GET  /api/static/*path          → tower ServeDir (BASE_PATH — filesystem)
```

All routes that return an error render the `error.html` template (HTML clients) or return a JSON `{ "error": "..." }` body (API clients).

---

## 6. API Handlers

All handlers live under `src/server/handlers/`. Each file corresponds to a resource domain.

### 6.1 Recipe handlers (`src/server/handlers/recipes.rs`)

#### `all_recipes` — `GET /api/recipes`

Calls `cooklang_find::build_tree(base_path)` to recursively scan the filesystem and returns a JSON tree of every `.cook` file found, preserving the directory hierarchy.

#### `recipe` — `GET /api/recipes/*path`

1. Validates path (no `..`, not absolute).
2. Reads the `.cook` file.
3. Parses with the Cooklang parser (lenient mode).
4. Applies optional `?scale=<f64>` scaling to ingredient quantities.
5. Groups ingredients by name (merge duplicates across sections).
6. Resolves the recipe image: if it's a local path it is rewritten to `/api/static/...`; external URLs are kept verbatim.
7. Returns:

```json
{
  "recipe": { /* full parsed recipe object */ },
  "image": "/api/static/Pasta/hero.jpg",
  "scale": 2.0,
  "grouped_ingredients": [
    { "index": 0, "quantities": [{ "value": 200, "unit": "g" }] }
  ]
}
```

#### `recipe_raw` — `GET /api/recipes/raw/*path`

Returns the raw file content as `text/plain`. Tries `.cook` first, then `.menu`.

#### `recipe_save` — `PUT /api/recipes/*path`

Atomic write:

1. Write new content to `<file>.tmp`
2. `rename()` the tmp file over the target

The temp file is removed on error via a fire-and-forget `tokio::spawn`.

#### `recipe_delete` — `DELETE /api/recipes/*path`

Removes the file; returns 404 if it does not exist.

#### `search` — `GET /api/search?q=<query>`

Delegates to `cooklang_find::search(base_path, query)` for full-text search. Returns an array of `{ "name", "path" }` objects.

#### `reload` — `GET|POST /api/reload`

No-op (the server reads from disk on every request; there is no cache to invalidate). Returns `{ "status": "ok" }`.

---

### 6.2 Shopping-list handlers (`src/server/handlers/shopping_list.rs`)

#### `shopping_list` — `POST /api/shopping_list`

Computes a shopping list on the fly from a list of recipe + scale pairs:

```json
[
  { "recipe": "Italian/Pasta.cook", "scale": 2.0 },
  { "recipe": "Sides/Salad.cook",   "scale": 1.0 }
]
```

Processing steps:

1. Parse each recipe file.
2. Scale ingredient quantities.
3. Merge ingredients with the same name (unit-aware).
4. If `aisle.conf` is present, group ingredients into shopping categories (e.g. "Produce", "Dairy").
5. If `pantry.conf` is present, subtract pantry inventory from quantities and mark pantry-sourced items.
6. Return:

```json
{
  "categories": [
    {
      "category": "Produce",
      "items": [
        { "name": "tomato", "quantities": [{ "value": 4, "unit": "" }] }
      ]
    }
  ],
  "pantry_items": ["salt", "olive oil"]
}
```

#### `get_shopping_list_items` — `GET /api/shopping_list/items`

Reads the persistent shopping list from `.shopping_list.txt` in `base_path`. Returns an array of `ShoppingListItem` objects (`path`, `name`, `scale`).

#### `add_to_shopping_list` — `POST /api/shopping_list/add`

Appends a `ShoppingListItem` to the persistent file. Duplicates are allowed (the same recipe can appear multiple times at different scales).

#### `remove_from_shopping_list` — `POST /api/shopping_list/remove`

Removes the **first** entry whose `path` matches. Allows incremental removal when the same recipe is listed multiple times.

#### `clear_shopping_list` — `POST /api/shopping_list/clear`

Truncates `.shopping_list.txt` to an empty list (keeps the comment header).

---

### 6.3 Pantry handlers (`src/server/handlers/pantry.rs`)

The pantry is persisted in `pantry.conf` (TOML). Items can be simple (`salt = true`) or rich:

```toml
salt = true
olive_oil = "1l"

[Produce]
tomato = { quantity = "2kg", expire = "2025-04-10" }
carrot = { low = "1kg" }   # needs restocking
```

#### `get_pantry` — `GET /api/pantry`

Parses `pantry.conf` and returns the full `PantryConf` structure.

#### `add_item` — `POST /api/pantry/add`

```json
{ "section": "Produce", "name": "carrot", "quantity": "2kg", "expire": "2025-05-01" }
```

Adds the item under the specified section (or the top-level section if `section` is `""`). Rewrites `pantry.conf`.

#### `update_item` — `PUT /api/pantry/:section/:name`

Partial update: only the fields present in the JSON body are changed; others are preserved. Automatically upgrades a simple `name = true` entry to an attributed object if needed.

#### `remove_item` — `DELETE /api/pantry/:section/:name`

Deletes the item. If the section becomes empty after removal, the section header is also removed.

#### `get_expiring` — `GET /api/pantry/expiring?days=7`

Returns items whose `expire` date is within the next `days` days (default 7). Supported date formats: `YYYY-MM-DD`, `DD.MM.YYYY`, `DD/MM/YYYY`, `MM/DD/YYYY`, `YYYY.MM.DD`, `DD-MM-YYYY`. Results are sorted from most urgent to least urgent.

```json
[
  { "section": "Dairy", "name": "milk", "expire": "2025-04-05", "days_remaining": 3 }
]
```

#### `get_depleted` — `GET /api/pantry/depleted`

Returns items with a non-null `low` attribute (items marked as running low / needing restock).

---

### 6.4 Menu handlers (`src/server/handlers/menus.rs`)

Menus are `.menu` files that use a Cooklang-like syntax to group recipe references and free-form ingredients under dated sections and meal types:

```
# Day 1 (2025-03-04):
## Breakfast (08:30):
- @Oatmeal.cook
- @berries{100g}

## Lunch:
- @Pasta.cook{2}
```

#### `list_menus` — `GET /api/menus`

Recursively scans `base_path` for `.menu` files. Returns `[{ "name", "path" }]`.

#### `get_menu` — `GET /api/menus/*path?scale=<f64>`

Parsing logic:

1. Level-1 headings (`# …`) become **sections**. A date in parentheses is extracted: `Day 1 (2025-03-04)` → `date = "2025-03-04"`.
2. Level-2 headings (`## …`) become **meals**. Text before the colon is the meal type; optional time in parentheses is extracted: `Breakfast (08:30):` → `type = "Breakfast"`, `time = "08:30"`.
3. List items (`- @Recipe.cook{scale}`) become `recipe_reference` items under the current meal.
4. Free ingredients inside a meal are parsed as `ingredient` items.

Response shape:

```json
{
  "name": "Weekly Menu",
  "path": "menus/march.menu",
  "metadata": { "servings": "4" },
  "sections": [
    {
      "name": "Day 1 (2025-03-04)",
      "date": "2025-03-04",
      "meals": [
        {
          "type": "Breakfast",
          "time": "08:30",
          "items": [
            {
              "kind": "recipe_reference",
              "name": "Oatmeal",
              "path": "Oatmeal.cook",
              "scale": 1.0
            }
          ]
        }
      ]
    }
  ]
}
```

---

### 6.5 Stats handler (`src/server/handlers/stats.rs`)

#### `stats` — `GET /api/stats`

Aggregates counts in a single pass:

```json
{
  "recipe_count": 42,
  "menu_count": 3,
  "pantry_item_count": 150,
  "pantry_expiring_count": 5,
  "pantry_depleted_count": 2
}
```

---

### 6.6 Sync handlers (`src/server/handlers/sync.rs`) — feature-gated

These endpoints are only compiled when the `sync` Cargo feature is enabled.

#### `sync_status` — `GET /api/sync/status`

```json
{ "logged_in": true, "email": "user@example.com", "syncing": false }
```

#### `sync_login` — `POST /api/sync/login`

1. Checks `login_in_progress` (atomic flag) to prevent concurrent login flows.
2. Opens a local TCP listener on a random port for the OAuth2 callback.
3. Spawns a background task that waits for the callback, exchanges the code for a token, and persists the session.
4. Returns `{ "login_url": "https://…" }` so the browser can redirect to the OAuth provider.

#### `sync_logout` — `POST /api/sync/logout`

Revokes the token and deletes the session file.

---

## 7. UI Page Handlers (`src/server/ui.rs`)

These handlers return fully rendered HTML pages. They call the same business logic as the API handlers but feed the results into Askama template structs.

| Handler | Route | Template |
|---|---|---|
| `recipes_page` | `GET /` | `recipes.html` |
| `recipes_directory` | `GET /directory/*path` | `recipes.html` |
| `recipe_page` | `GET /recipe/*path` | `recipe.html` |
| `edit_page` | `GET /edit/*path` | `edit.html` |
| `new_page` | `GET /new` | `new.html` |
| `create_recipe` | `POST /new` | — (redirects to `/recipe/*path`) |
| `menu_page_handler` | `GET /menu/*path` | `menu.html` |
| `shopping_list_page` | `GET /shopping-list` | `shopping_list.html` |
| `pantry_page` | `GET /pantry` | `pantry.html` |
| `preferences_page` | `GET /preferences` | `preferences.html` |

**`recipes_page` / `recipes_directory`**

- Builds a recipe directory tree from `cooklang_find::build_tree`.
- For the home page (`/`), checks whether a menu file with today's date exists and populates `todays_menu`.
- Extracts tags and the first image from each recipe for display in recipe cards.
- Constructs breadcrumbs for the current sub-path.

**`recipe_page`**

- Parses and scales the recipe.
- Merges duplicate ingredients across all sections.
- If `aisle.conf` is configured, reorders ingredients in aisle order for cooking mode.
- Resolves the title image path.

**`edit_page`**

- Loads the raw `.cook` source.
- Template includes a CodeMirror editor wired to the LSP WebSocket.

**`pantry_page`**

- Loads the full pantry, expiring items, and depleted items in one pass.

---

## 8. HTML Templates

### 8.1 Template engine

[Askama](https://github.com/djc/askama) compiles templates at **build time** into Rust code. Each template struct implements the `Template` trait and can call `render()` to produce a `String`. Template files live in `templates/` and are resolved relative to the workspace root via `askama.toml`.

Data is passed by populating the template struct fields before calling `.render()`. Templates can call Rust functions through **filters** (e.g. the `hostname` filter that extracts a domain from a URL).

### 8.2 Template data structures (`src/server/templates.rs`)

Key types:

| Struct | Used by template | Purpose |
|---|---|---|
| `Tr` | all templates | Translation helper; exposes `tr.t("key")` |
| `ErrorTemplate` | `error.html` | Single `error_message` string |
| `RecipesTemplate` | `recipes.html` | Items list, breadcrumbs, `todays_menu` |
| `RecipeTemplate` | `recipe.html` | Full recipe + scale + grouped ingredients |
| `EditTemplate` | `edit.html` | Raw source text |
| `NewTemplate` | `new.html` | (minimal, just `tr`) |
| `MenuTemplate` | `menu.html` | Parsed menu sections |
| `ShoppingListTemplate` | `shopping_list.html` | Rendered shopping categories |
| `PantryTemplate` | `pantry.html` | Full pantry + expiring + depleted |
| `PreferencesTemplate` | `preferences.html` | Config paths, version, sync info |
| `Breadcrumb` | navigation | `{ name, path }` |
| `RecipeItem` | recipe list | `{ name, path, tags, image, is_dir }` |
| `TodaysMenu` | home page | `{ menu_name, menu_path, date_display }` |

The `Tr` struct wraps a `LanguageIdentifier` and delegates lookups to `fluent-templates`. Every template receives a `tr` field so it can call `{{ tr.t("some_key") }}` for any user-visible string.

### 8.3 Template files

#### `base.html`

The master layout. All other templates extend it via Askama's block inheritance. Includes:

- Top navigation bar with links to Recipes, Shopping List, Pantry, Preferences
- Search bar (calls `GET /api/search`)
- Footer
- `<link>` and `<script>` tags for CSS and JS assets from `/static/`
- Language switcher (sets `lang` cookie, reloads page)
- Sync status indicator (feature-gated)

#### `recipes.html`

Directory browser. Renders:

- Breadcrumb trail for nested directories
- A "Today's Menu" card when a menu matching today's date exists
- Recipe cards with name, tags, and preview image
- Sub-directory cards that link to `/directory/*path`

#### `recipe.html`

Single recipe display. Renders:

- Title, servings, time, difficulty metadata
- Scale input (JavaScript sends `?scale=` and reloads)
- Grouped ingredients table
- Numbered step list with ingredient/cookware/timer inline highlights
- Cooking mode toggle (hides ingredients, highlights current step)
- Add-to-shopping-list button (calls `/api/shopping_list/add`)

#### `edit.html`

In-browser recipe editor. Uses CodeMirror with a Cooklang syntax-highlighting mode. On save, issues a `PUT /api/recipes/*path` request with the editor contents.

#### `menu.html`

Renders a menu file as sections (days) and meals (breakfast, lunch, dinner, …) with lists of recipe references and loose ingredients.

#### `shopping_list.html`

The shopping list page. JavaScript fetches `GET /api/shopping_list/items` on load, then calls `POST /api/shopping_list` with the stored items to compute the merged ingredient list. Checkboxes for completion are client-side only (not persisted). The list can be cleared via `POST /api/shopping_list/clear`.

#### `pantry.html`

Full pantry management UI. Inline forms backed by the CRUD endpoints at `/api/pantry/*`. Expiring and depleted sections are surfaced at the top.

#### `preferences.html`

Shows configuration file paths, the CookCLI binary version, the sync account (feature-gated), and the active language selection.

#### `error.html`

Minimal error page showing `error_message`. Extends `base.html`.

---

## 9. Static-file Serving

Two distinct static-file systems run in parallel:

### Compiled-in assets (`/static/*`)

Non-recipe assets (CSS, JS, favicons) are embedded into the binary at compile time using [`rust-embed`](https://github.com/pyros2097/rust-embed):

```rust
#[derive(RustEmbed)]
#[folder = "static/"]
struct StaticFiles;
```

The `serve_static` handler resolves the path against `StaticFiles`, sets appropriate `Content-Type` headers, and returns the file bytes. This means a production binary is fully self-contained.

### Recipe assets (`/api/static/*`)

Images and other files stored alongside recipes (e.g. `Pasta/hero.jpg`) are served at runtime by Tower's `ServeDir` middleware, rooted at `base_path`. Recipe handlers that reference a local image rewrite the path to `/api/static/<relative_path>`.

---

## 10. Shopping-list Persistence (`src/server/shopping_list_store.rs`)

The `ShoppingListStore` struct wraps `base_path/.shopping_list.txt`. Format:

```
# Shopping List
# Format: path<TAB>name<TAB>scale

Italian/Pasta.cook	Pasta	2
Sides/Salad.cook	Salad	1
```

| Method | Behaviour |
|---|---|
| `load()` | Read file; skip blank lines and `#` comments; parse tab-separated fields |
| `save(items)` | Overwrite file with comment header + all items |
| `add(item)` | `load` → push → `save` (duplicates allowed) |
| `remove(path)` | `load` → remove **first** match → `save` |
| `clear()` | `save(&[])` |

All methods are synchronous (`std::fs`). The file is stored in `base_path` rather than `/tmp/` so it persists across server restarts.

---

## 11. Internationalisation (i18n)

### 11.1 Language detection (`src/server/language.rs`)

An Axum middleware layer runs before every request:

1. Reads the `lang` cookie (set by the language-switcher in the UI).
2. Falls back to the `Accept-Language` HTTP header.
3. Falls back to English (`en-US`).
4. Stores the resolved `LanguageIdentifier` in the Axum request extensions so handlers can extract it.

Language can be changed by the user in **Preferences**; the UI sets the `lang` cookie and reloads the page.

### 11.2 Translation lookup (`src/server/i18n.rs`)

Uses [`fluent-templates`](https://github.com/XAMPPRocky/fluent-templates) with translation files in `locales/`. The `LOCALES` static is a lazy-initialised `fluent_templates::StaticLoader` that maps language identifiers to Fluent message bundles.

The `Tr` helper struct (in `templates.rs`) wraps the `LanguageIdentifier` and exposes `.t(key)` for use in templates:

```html
<h1>{{ tr.t("recipes-title") }}</h1>
```

---

## 12. LSP Bridge (`src/server/lsp_bridge.rs`)

The browser-based recipe editor communicates with a **Cooklang Language Server** for syntax checking and auto-complete. Because browsers cannot spawn processes, the server acts as a WebSocket-to-stdio bridge:

```
Browser WebSocket ──► lsp_websocket() handler
                           │
                      spawn child process: cook lsp
                           │
                      stdin/stdout ◄──► child process
```

When a WebSocket connection arrives at `GET /api/ws/lsp`:

1. The handler spawns `cook lsp` as a child process with piped stdin/stdout.
2. Two Tokio tasks are spawned:
   - **WS → LSP**: reads messages from the WebSocket and writes them to the process stdin.
   - **LSP → WS**: reads lines from the process stdout and sends them as WebSocket text messages.
3. When either side closes (browser tab closed, or LSP exits), both tasks are shut down.

A channel buffer of 32 messages is used between the two tasks.

---

## 13. Sync Subsystem (`src/server/sync/`)

Only compiled with the `sync` Cargo feature. Enables optional cloud synchronisation of recipes with the Cooklang cloud service.

| File | Responsibility |
|---|---|
| `mod.rs` | Types: `SyncSession`, `SyncHandle` |
| `session.rs` | Persist/load the OAuth2 session token from disk |
| `endpoints.rs` | OAuth2 callback listener (opens random port, waits for browser redirect) |
| `runner.rs` | Background sync task: push/pull recipe files; uses `shutdown_token` for graceful cancellation |

The background sync task is started at server boot if a saved session exists, and cancelled on logout or server shutdown.

---

## 14. Error Handling

Handlers use `anyhow::Result` internally. Before returning to Axum, errors are converted:

- **HTML routes** (`ui.rs`): on error, render `ErrorTemplate` with the error message and return `500 Internal Server Error`.
- **API routes** (`handlers/`): return `(StatusCode, Json({ "error": "…" }))`.
- **Path validation failures**: return `400 Bad Request` immediately.
- **File not found**: return `404 Not Found`.

The `handlers/common.rs` module provides shared helper types and the `validate_path()` function that rejects paths containing `..` or starting with `/` before any filesystem access.

---

## 15. Data-flow Diagrams

### Request: view a recipe

```
Browser GET /recipe/Italian/Pasta
    │
    ▼
Axum router → ui::recipe_page(State<AppState>, Path, Query, Language)
    │
    ├─ validate path (no "..", not absolute)
    ├─ read file: base_path/Italian/Pasta.cook  (std::fs::read_to_string)
    ├─ cooklang::parse(content)  →  ParsedRecipe
    ├─ apply scale factor
    ├─ group & merge ingredients
    ├─ resolve image path
    ├─ build RecipeTemplate { recipe, scale, ingredients, tr, ... }
    └─ RecipeTemplate::render()  →  HTML string
    │
    ▼
HTTP 200 text/html
```

### Request: compute shopping list

```
Browser POST /api/shopping_list   body: [{ recipe, scale }, ...]
    │
    ▼
handlers::shopping_list::shopping_list(State<AppState>, Json(items))
    │
    ├─ for each item:
    │   ├─ read + parse recipe file
    │   └─ scale ingredients
    ├─ merge all ingredients by name (unit-aware)
    ├─ load aisle.conf  →  categorise items
    ├─ load pantry.conf  →  subtract pantry quantities
    └─ return Json { categories, pantry_items }
    │
    ▼
HTTP 200 application/json
```

### Request: add pantry item

```
Browser POST /api/pantry/add   body: { section, name, quantity, expire }
    │
    ▼
handlers::pantry::add_item(State<AppState>, Json(req))
    │
    ├─ load pantry.conf  (toml::from_str)
    ├─ insert item into section
    ├─ serialize back to TOML
    └─ write pantry.conf  (std::fs::write)
    │
    ▼
HTTP 200 application/json  { "status": "ok" }
```

### WebSocket: editor LSP session

```
Browser WebSocket connect /api/ws/lsp
    │
    ▼
lsp_bridge::lsp_websocket(ws: WebSocket, State<AppState>)
    │
    ├─ spawn process: cook lsp
    │       stdin pipe ◄────┐
    │       stdout pipe ────┘
    │
    ├─ task A: WebSocket → process stdin
    │       ws.recv() → child.stdin.write()
    │
    └─ task B: process stdout → WebSocket
            child.stdout.read_line() → ws.send()
```
