# Arelang MVP Roadmap

## HTTP MVP Status

Status: complete as of 2026-05-27.

The HTTP MVP is closed when `scripts/mvp-smoke.sh` passes. That gate covers Rust formatting, Rust tests, clippy, Arelang formatting, `are check`, `are inspect`, `are openapi` export and drift checks, `are audit`, `are test`, generated minimal and users-template projects, and real HTTP smoke requests against local servers.

The next roadmap phase starts after the MVP and should focus on language identity, domain primitives, richer backend syntax, production safety, persistence, and packaging.

## North Star

Build the smallest Arelang implementation that can run a real HTTP `users_api` service.

The first version interprets Arelang instead of compiling to native code. The important part is that the language surface, diagnostics, and backend flow feel like the intended product.

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
- parser recovery for common syntax errors: done for MVP diagnostics; broader recovery deferred
- parser support/test module split: done

Initial language items:

- `use`
- `struct`
- `enum`
- `type`
- `fn`
- `service`
- `route` legacy form
- `get`/`post` method shorthand inside services
- `let`
- `return`
- blocks and expressions

Definition of done:

- `are check examples/users_api` parses all source files
- syntax errors return human-readable and JSON diagnostics
- duplicate declarations and missing route handlers return resolver diagnostics

## Milestone 2: Resolver And Type Checker

Output:

- module/import resolution: done for MVP builtins and single-package sources; full package imports deferred
- nominal struct and enum declaration checks: done for MVP declarations
- function signature checks: done for MVP handler and local-call signatures
- `Result<T, E>` arity checks: done
- `Option<T>` arity checks: done
- HTTP route handler signature checks: done
- HTTP error mapper checks: done
- declarative HTTP error contract checks through `Http.errors(ApiError)`: done after HTTP MVP
- `?` propagation checks: done
- route body contract checks: done
- route query contract checks: done
- route headers contract checks: done
- route cookies contract checks: done
- route contract handler parameter binding: done
- declarative struct field validation checks for `validate.email` and `validate.length`: done
- declarative domain alias validation checks for `validate.email` and `validate.length`: done after HTTP MVP
- route response and success status contract checks: done
- route handlers returning domain payloads from `returns` contracts: done
- model database call checks for `ctx.db.<collection>.insert/get`: done for MVP model collections; broader query APIs deferred
- typed path parameter contract checks: done
- function body checker module split: done

Definition of done:

- invalid field names, unknown symbols, bad return types, and unhandled `Result` values are reported with stable diagnostic codes
- `users_api` route handlers may bind route inputs directly as typed parameters such as `input: CreateUserInput`, `id: UserId`, and `cookies: SessionCookies`

## Milestone 3: Interpreter Core

Output:

- expression interpreter: done for the MVP function slice
- function calls: done for the MVP function slice
- structs/enums at runtime: done for MVP JSON payloads, errors, and enum constructors
- in-memory values: done for JSON, booleans, HTTP responses, enums, and unit
- basic standard library hooks: done for MVP HTTP, validation, and model-store flows
- interpreter value/error/host module split: done

Definition of done:

- simple Arelang functions can execute without HTTP
- interpreter and type checker agree on function boundaries

## Milestone 4: HTTP Runtime

Status: complete for the HTTP MVP.

Output:

- `service` route registry: done for one-service MVP projects
- checked HTTP contract manifest: done for service, routes, body, query, headers, cookies, response, status, route error types, typed params, handlers, local schemas, declarative error contract, and compatibility error mapper
- local server runner: done for `users_api`
- request/response runtime types: done for the MVP HTTP boundary
- JSON decode/encode MVP: done for local structs/models with primitive fields
- model-backed in-memory store: done for primary-key `insert/get`
- route params: done for legacy `:id` and typed `{id: UserId}` forms
- route body contracts: done for `body Payload`
- route query contracts: done for `query Payload`
- route headers contracts: done for `headers Payload`
- route cookies contracts: done for `cookies Payload`
- route contract parameter binding: done for body, query, headers, cookies, and typed path params
- route response contracts: done for `returns Payload status N`
- declarative request payload validation: done for body, query, headers, and cookies
- domain payload handlers: done for `Payload` and `Result<Payload, E>`
- API error mapping: done through `Http.errors(ApiError)` enum status metadata and compatibility `Http.error_map`

Definition of done:

- `are run examples/users_api` starts a local HTTP server: done
- `GET /health` returns `200`: done
- `POST /users` creates an in-memory user: done
- `GET /users/{id: UserId}` returns that user or a typed error: done
- canonical route contracts with method shorthand, typed path params, request body declarations, request query declarations, request headers declarations, and request cookies declarations: done
- successful HTTP responses are validated against declared `returns` and `status` contracts: done
- successful domain payloads are wrapped into HTTP responses by the route contract: done
- function body interpreter replaces the temporary users API adapter: done
- `ctx.db.users` is resolved from `model User` rather than a users-only runtime adapter: done
- HTTP runtime is split into manifest, server, response, schema, host, store, scenario, and test modules: done

## Milestone 5: Backend Quality Loop

Status: complete for the HTTP MVP.

Output:

- `are fmt`: done for parsed MVP syntax
- `are test`: done for built-in MVP scenarios
- `are inspect`: done for checked HTTP contract manifest output
- `are inspect` schema export: done for aliases, structs, models, enum variants, enum HTTP statuses, route error types, model collections, and primary/unique field metadata
- `are inspect` field validation export: done for struct field validation metadata
- `are inspect` alias validation export: done for domain primitive validation metadata
- `are openapi`: done for OpenAPI 3.1 JSON paths, request bodies, success/error responses, path/query/header/cookie params, alias and field validation constraints, declarative error contract responses, servers, component schemas, file output, and drift checks
- `are check --json`: done
- diagnostic fix suggestions: done for MVP name, type, handler, mapper, field, and enum-variant diagnostics
- source snippet diagnostics: done
- users API tests: done through built-in scenarios, generated-template smoke, and real HTTP smoke
- generated users API template: done
- MVP smoke script in CI: done
- pinned Rust toolchain for reproducible CI: done

Definition of done:

- generated code can be checked, formatted, inspected, run, and tested in one loop: done

## Milestone 6: Safety And Production Shape

Status: post-MVP started.

Output:

- declarative domain error contracts: done for HTTP status metadata and automatic error response mapping
- request scope model
- first arena/region checker
- capability manifest checks: started for HTTP MVP listen/process/default-closed capabilities
- audit command seed: done
- structured logging and metrics defaults

Definition of done:

- the toolchain can catch at least one real backend safety class: started with undeclared HTTP listen capability and process-spawn capability checks
- the compiler can catch at least one request-scope safety class, such as storing request-scoped data in process state

## Deferred Until After HTTP MVP

- native codegen
- LLVM/Cranelift backend
- database adapter
- worker pools
- full sandbox runtime
- package registry
- self-hosting
