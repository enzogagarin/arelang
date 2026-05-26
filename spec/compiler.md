# Compiler Architecture Seed

Arelang starts as an interpreter-first compiler toolchain. The first mission is a working HTTP server, not optimized native codegen.

## Bootstrap Language

Rust.

Reasons:

- strong enum and pattern matching support for AST and IR
- safe compiler internals
- good test tooling
- good fit for diagnostics and future region analysis

## Initial Crates

```text
crates/
  are-ast/           shared syntax tree data model
  are-cli/           CLI entry point; owns commands like check, run, fmt
  are-diagnostics/   human and JSON diagnostic data model
  are-lexer/         source text to token stream
  are-parser/        token stream to top-level AST, with parser support/test modules
  are-project/       manifest loading and reusable static check pipeline
  are-resolver/      top-level symbol binding and service route checks
  are-typecheck/     type arity, duplicate fields, function-body checks, and HTTP service contracts
  are-interpreter/   MVP function interpreter split into value, error, host, and runner modules
  are-http-runtime/  first HTTP MVP server for checked service projects
```

Next crates:

```text
crates/
  are-runtime/
```

## First Pass Pipeline

```text
source.are
  -> lexer
  -> parser
  -> resolver
  -> type checker
  -> service registry
  -> interpreter/runtime
  -> HTTP server
```

## CLI Contract

```sh
are check examples/users_api
are check --json examples/users_api
are fmt examples/users_api --check
are test examples/users_api
are run examples/users_api
```

`are check --json` is the AI-agent contract. Its schema must remain stable once parser diagnostics begin.

Current `are check` behavior:

- collect `.are` files
- lex each file
- parse top-level items into AST
- resolve imports, declarations, service uses, and route handlers
- typecheck function signatures, generic arity, route handlers, route body contracts, route response/status contracts, typed path params, and HTTP error mappers
- resolve model database calls such as `ctx.db.users.insert` from local `model User` declarations
- emit human or JSON diagnostics

Compiler implementation hygiene:

- Rust is pinned through `rust-toolchain.toml` to keep local and CI behavior reproducible.
- `are-parser` keeps grammar parsing in `lib.rs` and token/diagnostic helpers in `support.rs`.
- `are-typecheck` keeps declaration orchestration in `lib.rs`, function-body checking in `body.rs`, HTTP route contracts in `http.rs`, and regression tests in `tests.rs`.
- `are-interpreter` keeps the public runtime values, host boundary, and error model in separate modules before the evaluator grows further.

Current `are fmt` behavior:

- collect `.are` files from a project directory or format one `.are` file
- parse the supported MVP syntax into AST
- render canonical spacing, indentation, and top-level grouping
- refuse comment-containing files until comments can be preserved
- support `--check` for CI

Current `are run` behavior:

- load `are.toml`
- require `target = "server"`
- run static checks
- extract the checked service route registry
- wrap domain payload handler results using the route response/status contract
- persist MVP model data through the model-backed in-memory store
- validate route-level body, path, response, and status contracts at the host boundary
- start the embedded HTTP MVP runtime

Current `are test` behavior:

- run the same static checks and runtime project preparation as `are run`
- collect the checked service route registry
- expose route body, response, status, typed path param, and handler data in the test report
- execute built-in MVP runtime scenarios without opening a TCP listener
- emit human or JSON test reports

## Diagnostic Shape

Diagnostics should include:

- code
- severity
- file
- range
- problem
- reason
- fixes

Human diagnostic output should render the same payload with a source snippet:

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

Security-impacting fixes should not be automatically applied.
