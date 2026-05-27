# Arelang

[![MVP Smoke](https://github.com/enzogagarin/arelang/actions/workflows/mvp-smoke.yml/badge.svg)](https://github.com/enzogagarin/arelang/actions/workflows/mvp-smoke.yml)

Arelang is a small backend-first programming language for AI-assisted service generation.

The first product goal is not a complete general-purpose language. The first goal is a working HTTP server version that can run a small `users_api` service with typed request/response models, clear errors, and a deterministic compiler feedback loop.

## HTTP MVP Status

The HTTP MVP is complete. The current closure gate is:

```sh
./scripts/mvp-smoke.sh
```

That gate verifies the Rust workspace, Arelang static checks, formatter, contract inspection, OpenAPI export and drift checks, audit checks, built-in scenario tests, generated projects, and real local HTTP requests. The remaining roadmap is post-MVP work: stronger language identity, richer domain syntax, production safety, persistence, packaging, and eventually native codegen.

## Initial Direction

- File extension: `.are`
- Package manifest: `are.toml`
- Bootstrap implementation: Rust compiler/runtime
- First execution model: interpreter-first, with an embedded HTTP runtime
- First demo: `examples/users_api`
- Error model: `Result<T, E>` plus `?` propagation
- Absence model: `Option<T>`
- Syntax style: braces, no semicolons, mandatory formatter
- Backend target: HTTP server before native codegen

## Why This Shape

The language should optimize for clear generated backend code. That means avoiding hidden control flow, nulls, implicit casts, exceptions, macro-heavy frameworks, and large syntax variation.

The first useful version should answer this question:

> Can Arelang run a real HTTP users API with validation, JSON, structured errors, and predictable diagnostics?

Native codegen, arena escape checking, package publishing, database adapters, and sandbox policy are important, but they come after the first server works.

## Planned Commands

```sh
are fmt
are check --json
are inspect --json
are openapi examples/users_api --output openapi.json
are openapi examples/users_api --check --output openapi.json
are run examples/users_api
are test
are audit
are build --release
```

`are run examples/users_api` is the first command that should feel real.

## Current Bootstrap Commands

From the repository root, the current demos are intentionally a small set of commands:

```sh
./are new scratch_api --port 8090
./are new scratch_users --template users --port 8091
./are fmt scratch_api --check
./are check scratch_api
./are inspect scratch_api
./are openapi scratch_api --output scratch_api/openapi.json
./are openapi scratch_api --check
./are audit scratch_api
./are test scratch_api
./are run scratch_api
./are fmt examples/hello_api --check
./are check examples/hello_api
./are audit examples/hello_api
./are test examples/hello_api
./are run examples/hello_api
./are fmt examples/users_api --check
./are check examples/users_api
./are check examples/users_api --json
./are inspect examples/users_api
./are inspect examples/users_api --json
./are openapi examples/users_api --output openapi.json
./are openapi examples/users_api --check --output openapi.json
./are audit examples/users_api
./are audit examples/users_api --json
./are test examples/users_api
./are test examples/users_api --json
./are run examples/users_api
./scripts/mvp-smoke.sh
cargo test
```

To install the local CLI as `are`, run:

```sh
cargo install --path crates/are-cli
```

After that, the commands become:

```sh
are check examples/hello_api
are inspect examples/hello_api
are audit examples/hello_api
are fmt examples/hello_api --check
are test examples/hello_api
are run examples/hello_api
```

`are new` creates an HTTP server project with an `are.toml` manifest and a `main.are` file. The default template starts with a single `GET /ping` route. The backend-first users template creates the fuller MVP shape:

```sh
./are new users_api --template users
./are run users_api
```

When a server starts, `are run` prints the service name, package, listen URL, and route table so the project is immediately curlable.

`scripts/mvp-smoke.sh` is the current MVP health check. It runs Rust formatting, Arelang formatting, Rust tests, clippy, `are check`, `are inspect`, `are openapi`, `are audit`, `are test`, generated minimal and users-template APIs, real HTTP servers on high local ports, and response verification with `curl`.

`are fmt` rewrites `.are` files into the canonical Arelang style. `--check` verifies formatting without writing, which is what CI uses. The first formatter intentionally refuses to rewrite files with comments until comment-preserving formatting is implemented.

`are check` currently lexes, parses, resolves top-level symbols, and typechecks the first HTTP service contract rules. Human diagnostics include source snippets and `help:` suggestions for nearby names, while `--json` keeps the structured diagnostic payload for tools and CI. The parser now also builds a minimal function-body AST for `let`, `return`, `ensure`, `match`, `?`, generic calls, enum constructors, object literals, field paths, booleans, and named arguments. It also understands `model` declarations with field attributes such as `primary` and `unique`, plus struct field validations such as `validate.email` and `validate.length(min: 2, max: 80)`. Function bodies get semantic checks for local function calls, enum match coverage, std HTTP calls, typed route contract parameters, request JSON/query/header/cookie decoding, validation, route params, database access, return types, route request/response/status contracts, and `?` usage.

`are test` runs the project quality loop without opening a TCP listener. It reuses the same static check and HTTP runtime preparation as `are run`, then executes built-in MVP scenarios for known backend shapes such as `GET /ping` and the users API flow. `--json` emits a machine-readable test report.

The HTTP runtime now prepares a checked contract manifest before serving requests. That manifest is the single runtime view of the service name, method/path pairs, typed path params, request body type, query type, headers type, cookies type, response type, success status, handler binding, local type/model/enum schemas, field validations, and error mapper. `are run` uses it to route and validate requests/responses; `are test --json` exposes the same route contract data for tools.

`are inspect` prints the same checked HTTP contract manifest without running a server or executing built-in scenarios. `--json` emits the manifest directly for tools that need the API surface, including aliases, structs, field validation metadata, models, enum variants, model collections, and primary/unique model field metadata. This is the seed for OpenAPI/client generation and lets Arelang expose backend contracts without making users read the runtime internals.

`are openapi` exports the checked HTTP contract as OpenAPI 3.1 JSON. It maps service routes to `paths`, body/response contracts to request and response schemas, typed path params plus typed query/header/cookie contracts to OpenAPI parameters, field validations to schema constraints such as `format: email`, `minLength`, and `maxLength`, and Arelang aliases, structs, models, and enums to `components.schemas`. By default it prints to stdout; `--output openapi.json` writes a stable file, and `--check` fails when that file has drifted from the current Arelang source. Without `--output`, `--check` looks for `openapi.json` in the project root.

`are audit` is the first production-shape safety loop. It runs static checks, builds the HTTP contract manifest, verifies every route has a response type and success status, and checks `[capabilities]` in `are.toml` against the server listen address and the MVP least-privilege defaults. It fails on missing required listen capability, missing capability manifest, static check failures, or process spawning being enabled.

Example human diagnostic:

```text
error[E_RESOLVE_0002]: unknown route handler `create_usr`
  --> users_api/main.are:61:28
   |
61 |     post "/users" body CreateUserInput -> create_usr
   |                                           ^^^^^^^^^^
   |
note: declare a function with this name before wiring it in a service route
help: did you mean `create_user`?
```

`examples/hello_api` is the smallest runnable HTTP server. It listens on `127.0.0.1:8081` and responds to `GET /ping` from an Arelang function body.

```sh
curl http://127.0.0.1:8081/ping
```

`examples/users_api` is the first backend-shaped demo. It listens on `127.0.0.1:8080`. The `/health`, `POST /users`, `GET /users/search`, `GET /users/auth-check`, `GET /session`, and `GET /users/{id: UserId}` routes are executed from their Arelang function bodies through the MVP interpreter. Service routes now carry typed backend contracts directly, so handlers can receive HTTP inputs as ordinary typed parameters and return domain payloads without manually wrapping every success response:

```are
struct CreateUserInput {
    email: Email validate.email
    name: String validate.length(min: 2, max: 80)
}

struct SearchUsersQuery {
    email: Email validate.email
}

struct AuthHeaders {
    authorization: String validate.length(min: 7, max: 200)
}

struct SessionCookies {
    session_id: SessionId validate.length(min: 6, max: 120)
}

fn create_user(ctx: Http.Context<AppState>, input: CreateUserInput) -> Result<User, ApiError> {
    let user = ctx.db.users.insert(input)?
    return user
}

fn search_users(ctx: Http.Context<AppState>, query: SearchUsersQuery) -> Result<SearchUsersResponse, ApiError> {
    return { "email": query.email }
}

fn auth_check(ctx: Http.Context<AppState>, headers: AuthHeaders) -> Result<AuthCheckResponse, ApiError> {
    return { "authorized": true }
}

fn current_session(ctx: Http.Context<AppState>, cookies: SessionCookies) -> Result<SessionResponse, ApiError> {
    return { "session_id": cookies.session_id, "active": true }
}

fn get_user(ctx: Http.Context<AppState>, id: UserId) -> Result<User, ApiError> {
    let user = ctx.db.users.get(id)?
    return user
}
```

```are
service UsersApi(state: AppState) {
    use Http.error_map(map_error)

    get "/health" -> health returns HealthResponse status 200
    post "/users" body CreateUserInput -> create_user returns User status 201
    get "/users/search" query SearchUsersQuery -> search_users returns SearchUsersResponse status 200
    get "/users/auth-check" headers AuthHeaders -> auth_check returns AuthCheckResponse status 200
    get "/session" cookies SessionCookies -> current_session returns SessionResponse status 200
    get "/users/{id: UserId}" -> get_user returns User status 200
}
```

The compiler checks that `post "/users" body CreateUserInput` is bound by the handler as `input: CreateUserInput`, that `get "/users/search" query SearchUsersQuery` is bound as `query: SearchUsersQuery`, that `get "/users/auth-check" headers AuthHeaders` is bound as `headers: AuthHeaders`, that `get "/session" cookies SessionCookies` is bound as `cookies: SessionCookies`, that `returns User status 201` matches a `User` or `Result<User, ApiError>` handler return, and that `{id: UserId}` is bound as `id: UserId`. Struct validations are checked statically for compatible field types, enforced at the HTTP boundary for body/query/header/cookie payloads, exported through `are inspect`, and lowered into OpenAPI schema constraints. Runtime then wraps successful domain payloads with the route status, validates response JSON and success status, and sends the HTTP response. Manual `ensure` remains available for business rules, `model User` describes the persisted shape, and `ctx.db.users.insert/get` is resolved through the model-backed in-memory store before `Http.error_map(map_error)` maps errors to HTTP responses with an Arelang `match`. The lower-level `req.json<T>()`, `req.query<T>()`, `req.headers<T>()`, `req.cookies<T>()`, and `ctx.param<T>()` calls remain available for compatibility and escape hatches.

```sh
curl http://127.0.0.1:8080/health
curl -X POST http://127.0.0.1:8080/users \
  -H 'Content-Type: application/json' \
  -d '{"email":"ada@example.com","name":"Ada"}'
curl 'http://127.0.0.1:8080/users/search?email=ada%40example.com'
curl http://127.0.0.1:8080/users/auth-check -H 'authorization: Bearer dev-token'
curl http://127.0.0.1:8080/session -H 'Cookie: session_id=session-dev-123'
curl http://127.0.0.1:8080/users/1
```
