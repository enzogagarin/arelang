# Arelang

[![MVP Smoke](https://github.com/enzogagarin/arelang/actions/workflows/mvp-smoke.yml/badge.svg)](https://github.com/enzogagarin/arelang/actions/workflows/mvp-smoke.yml)

Arelang is a small backend-first programming language for AI-assisted service generation.

The first product goal is not a complete general-purpose language. The first goal is a working HTTP server version that can run a small `users_api` service with typed request/response models, clear errors, and a deterministic compiler feedback loop.

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
./are test scratch_api
./are run scratch_api
./are fmt examples/hello_api --check
./are check examples/hello_api
./are test examples/hello_api
./are run examples/hello_api
./are fmt examples/users_api --check
./are check examples/users_api
./are check examples/users_api --json
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

`scripts/mvp-smoke.sh` is the current MVP health check. It runs Rust formatting, Arelang formatting, Rust tests, clippy, `are check`, `are test`, generated minimal and users-template APIs, real HTTP servers on high local ports, and response verification with `curl`.

`are fmt` rewrites `.are` files into the canonical Arelang style. `--check` verifies formatting without writing, which is what CI uses. The first formatter intentionally refuses to rewrite files with comments until comment-preserving formatting is implemented.

`are check` currently lexes, parses, resolves top-level symbols, and typechecks the first HTTP service contract rules. Human diagnostics include source snippets and `help:` suggestions for nearby names, while `--json` keeps the structured diagnostic payload for tools and CI. The parser now also builds a minimal function-body AST for `let`, `return`, `ensure`, `match`, `?`, generic calls, enum constructors, object literals, field paths, booleans, and named arguments. It also understands `model` declarations with field attributes such as `primary` and `unique`. Function bodies get semantic checks for local function calls, enum match coverage, std HTTP calls, request JSON decoding, validation, route params, database access, return types, route response/status contracts, and `?` usage.

`are test` runs the project quality loop without opening a TCP listener. It reuses the same static check and HTTP runtime preparation as `are run`, then executes built-in MVP scenarios for known backend shapes such as `GET /ping` and the users API flow. `--json` emits a machine-readable test report.

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

`examples/users_api` is the first backend-shaped demo. It listens on `127.0.0.1:8080`. The `/health`, `POST /users`, and `GET /users/{id: UserId}` routes are executed from their Arelang function bodies through the MVP interpreter. Service routes now carry typed backend contracts directly, so handlers can return domain payloads instead of manually wrapping every success response:

```are
fn create_user(ctx: Http.Context<AppState>, req: Http.Request) -> Result<User, ApiError> {
    let input = validate_user(req.json<CreateUserInput>()?)?
    let user = ctx.db.users.insert(input)?
    return user
}
```

```are
service UsersApi(state: AppState) {
    use Http.error_map(map_error)

    get "/health" -> health returns HealthResponse status 200
    post "/users" body CreateUserInput -> create_user returns User status 201
    get "/users/{id: UserId}" -> get_user returns User status 200
}
```

The compiler checks that `post "/users" body CreateUserInput` is decoded by the handler with `req.json<CreateUserInput>()`, that `returns User status 201` matches a `User` or `Result<User, ApiError>` handler return, and that `{id: UserId}` is read as `ctx.param<UserId>("id")`. Runtime then wraps successful domain payloads with the route status, validates response JSON and success status, and sends the HTTP response. Validation can live in local Arelang functions, `ensure` can raise enum errors such as `ApiError.InvalidInput("invalid_email")`, `model User` describes the persisted shape, and `ctx.db.users.insert/get` is resolved through the model-backed in-memory store before `Http.error_map(map_error)` maps errors to HTTP responses with an Arelang `match`.

```sh
curl http://127.0.0.1:8080/health
curl -X POST http://127.0.0.1:8080/users \
  -H 'Content-Type: application/json' \
  -d '{"email":"ada@example.com","name":"Ada"}'
curl http://127.0.0.1:8080/users/1
```
