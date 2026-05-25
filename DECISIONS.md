# Arelang Decision Log

This file records early decisions so the language does not drift while the compiler is still small.

## D001: First Target Is A Working HTTP Server

Status: accepted

Arelang v0 will prioritize running an HTTP server over native code generation. The first milestone is a `users_api` example that can start a local server, expose health and user routes, parse JSON, return JSON, and surface structured errors.

Native binary output remains a core long-term goal, but it should not block the first usable backend loop.

## D002: Use `.are` As The Source Extension

Status: accepted

`.are` is short, readable, and tied directly to the Arelang name. It is also easy for AI agents and humans to recognize in project trees.

## D003: Use `are.toml` For The Manifest

Status: accepted

The original design considered `are.pkg`, but `are.toml` is better for the bootstrap phase:

- familiar to developers
- easy to parse with existing Rust crates
- stable and readable for AI-generated projects
- avoids inventing a manifest format before the language itself works

The CLI remains `are`.

## D004: Use Rust For The Bootstrap Compiler And Runtime

Status: accepted

Rust is the best fit for a compiler that needs AST enums, typed diagnostics, parser recovery, ownership-aware analysis, and a future region checker.

Go may be faster for a short prototype, but Rust better matches the long-term safety and compiler architecture goals.

## D005: Prefer Explicit `Result<T, E>` And `Option<T>` In v0

Status: accepted

The technical document proposed compact forms such as `T!E` and `T?`. For v0, Arelang should prefer the most understandable surface:

```are
fn create_user(req: Http.Request) -> Result<Http.Response, ApiError>
```

This is more verbose than `Response!ApiError`, but it is easier to read, easier to teach, and less surprising for generated code.

The `?` operator is still used for propagation.

## D006: Defer Explicit Arena Syntax Until After HTTP Works

Status: accepted

`uses request_arena` is powerful but premature for the first working server. v0 should make request scope visible through handler context and runtime conventions first.

The first implementation should not pretend to solve all memory safety. Region and arena escape checks become a dedicated milestone after the HTTP MVP.

## D007: First Showcase Is `users_api`

Status: accepted

The first real example is a users API because it exercises the important backend path:

- health check
- create user
- get user
- JSON request body
- JSON response body
- typed route params
- validation
- structured API errors

