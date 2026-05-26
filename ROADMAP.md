# Arelang MVP Roadmap

## North Star

Build the smallest Arelang implementation that can run a real HTTP `users_api` service.

The first version may interpret Arelang instead of compiling to native code. The important part is that the language surface, diagnostics, and backend flow feel like the intended product.

## Milestone 0: Planning And Spec Seed

Output:

- decision log
- syntax seed
- HTTP MVP spec
- `users_api` target example

Definition of done:

- the first syntax choices are written down
- the first demo is concrete enough to drive compiler work

## Milestone 1: CLI, Lexer, Parser

Output:

- Rust workspace: done
- `are check`: lexer + parser path done
- token stream tests: done
- AST model: done
- parser tests for `users_api`: done
- top-level resolver: done
- parser recovery for common syntax errors: started

Initial language items:

- `use`
- `struct`
- `enum`
- `type`
- `fn`
- `service`
- `route`
- `let`
- `return`
- blocks and expressions

Definition of done:

- `are check examples/users_api` parses all source files
- syntax errors return human-readable and JSON diagnostics
- duplicate declarations and missing route handlers return resolver diagnostics

## Milestone 2: Resolver And Type Checker

Output:

- module/import resolution: started
- nominal struct and enum declaration checks: started
- function signature checks: started
- `Result<T, E>` arity checks: done
- `Option<T>` arity checks: done
- HTTP route handler signature checks: done
- HTTP error mapper checks: done
- `?` propagation checks: pending

Definition of done:

- invalid field names, unknown symbols, bad return types, and unhandled `Result` values are reported with stable diagnostic codes
- `users_api` route handlers must use `(ctx: Http.Context<AppState>, req: Http.Request)` and return `Http.Response` or `Result<Http.Response, ApiError>`

## Milestone 3: Interpreter Core

Output:

- expression interpreter
- function calls
- structs/enums at runtime
- in-memory values
- basic standard library hooks

Definition of done:

- simple Arelang functions can execute without HTTP
- interpreter and type checker agree on function boundaries

## Milestone 4: HTTP Runtime

Output:

- `service` route registry: started
- local server runner: done for `users_api`
- request/response runtime types: started
- JSON decode/encode MVP
- route params
- API error mapping

Definition of done:

- `are run examples/users_api` starts a local HTTP server: done
- `GET /health` returns `200`: done
- `POST /users` creates an in-memory user: done
- `GET /users/:id` returns that user or a typed error: done
- function body interpreter replaces the temporary users API adapter

## Milestone 5: Backend Quality Loop

Output:

- `are fmt`
- `are test`
- `are check --json`
- diagnostic fix suggestions
- users API tests: started
- generated users API template: done
- MVP smoke script in CI: done

Definition of done:

- generated code can be checked, formatted, run, and tested in one loop

## Milestone 6: Safety And Production Shape

Output:

- request scope model
- first arena/region checker
- capability manifest checks
- audit command seed
- structured logging and metrics defaults

Definition of done:

- the compiler can catch at least one real backend safety class, such as storing request-scoped data in process state

## Deferred Until After HTTP MVP

- native codegen
- LLVM/Cranelift backend
- database adapter
- worker pools
- full sandbox runtime
- package registry
- self-hosting
