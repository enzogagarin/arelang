# Arelang

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

The repository currently includes the first Rust workspace and syntax-backed check command:

```sh
cargo run -p are-cli -- check examples/users_api
cargo run -p are-cli -- check examples/users_api --json
cargo test
```

`are check` currently lexes, parses, resolves top-level symbols, and typechecks the first HTTP service contract rules. The parser now also builds a minimal function-body AST for simple `return` expressions such as object literals and `Http.Response.ok(...)` calls.

`are run examples/users_api` now starts the first HTTP MVP server on `127.0.0.1:8080`. The `/health` route is executed from the Arelang `health` function body through the MVP interpreter. The `POST /users` and `GET /users/:id` handlers still use a temporary users API adapter while the richer function interpreter is built out.

```sh
curl http://127.0.0.1:8080/health
curl -X POST http://127.0.0.1:8080/users \
  -H 'Content-Type: application/json' \
  -d '{"email":"ada@example.com","name":"Ada"}'
curl http://127.0.0.1:8080/users/1
```
