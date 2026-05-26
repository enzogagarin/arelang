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

From the repository root, the current demos are intentionally a small set of commands:

```sh
./are new scratch_api --port 8090
./are check scratch_api
./are run scratch_api
./are check examples/hello_api
./are run examples/hello_api
./are check examples/users_api
./are check examples/users_api --json
./are run examples/users_api
cargo test
```

To install the local CLI as `are`, run:

```sh
cargo install --path crates/are-cli
```

After that, the commands become:

```sh
are check examples/hello_api
are run examples/hello_api
```

`are new` creates a minimal HTTP server project with an `are.toml` manifest and a `main.are` file. The generated project starts with a single `GET /ping` route.

`are check` currently lexes, parses, resolves top-level symbols, and typechecks the first HTTP service contract rules. The parser now also builds a minimal function-body AST for `let`, `return`, `?`, generic calls, object literals, field paths, and named arguments.

`examples/hello_api` is the smallest runnable HTTP server. It listens on `127.0.0.1:8081` and responds to `GET /ping` from an Arelang function body.

```sh
curl http://127.0.0.1:8081/ping
```

`examples/users_api` is the first backend-shaped demo. It listens on `127.0.0.1:8080`. The `/health`, `POST /users`, and `GET /users/:id` routes are executed from their Arelang function bodies through the MVP interpreter.

```sh
curl http://127.0.0.1:8080/health
curl -X POST http://127.0.0.1:8080/users \
  -H 'Content-Type: application/json' \
  -d '{"email":"ada@example.com","name":"Ada"}'
curl http://127.0.0.1:8080/users/1
```
