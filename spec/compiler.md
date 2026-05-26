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
  are-parser/        token stream to top-level AST
  are-project/       manifest loading and reusable static check pipeline
  are-resolver/      top-level symbol binding and service route checks
  are-typecheck/     type arity, duplicate fields, and HTTP service contract checks
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
- typecheck function signatures, generic arity, route handlers, and HTTP error mappers
- emit human or JSON diagnostics

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
- start the embedded HTTP MVP runtime

Current `are test` behavior:

- run the same static checks and runtime project preparation as `are run`
- collect the checked service route registry
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
61 |     route POST "/users" -> create_usr
   |                            ^^^^^^^^^^
   |
note: declare a function with this name before wiring it in a service route
help: did you mean `create_user`?
```

Security-impacting fixes should not be automatically applied.
